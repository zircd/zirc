//! What a wake says where people see it ([`zirc_proto::woke`]): for each wake of a
//! session's watch, one line from the session in the room its posts go to, saying what
//! happened and whose doing it was, never what anyone said. Apps show it in the session's
//! own conversation; it wakes no watch ([`zirc_proto::woke::is_kind`]).

use zirc_core::ipc::{Delivery, Direct, Response, Why};
use zirc_proto::woke::{By, What, Woke};
use zirc_proto::{Event, Message, ci};

use super::{Daemon, WATCHER_SESSION};

impl Daemon {
    /// Says, as `session`, that its watch woke on `response` for `why`: the wake `id`
    /// ([`Daemon::woke`]). In the scope of the repository the line was said in when the
    /// session works there, else of one it works in; nowhere for a session in none, which
    /// has no room to say it in.
    pub(super) fn say_woke(&mut self, session: &str, id: String, why: Why, response: &Response) {
        let said_in = match response {
            Response::Delivery(delivery) => Some(delivery.scope()),
            Response::Overlap(overlap) => Some(overlap.channel.clone()),
            Response::Conflict(conflict) => Some(conflict.channel.clone()),
            Response::LocalOverlap(overlap) => Some(overlap.channel.clone()),
            _ => None,
        };
        let scopes: Vec<&str> = self
            .repos
            .values()
            .filter(|watched| watched.is_of(session))
            .map(|watched| watched.scope.as_str())
            .collect();
        let Some(scope) = scopes
            .iter()
            .find(|scope| said_in.as_deref() == Some(**scope))
            .or_else(|| scopes.first())
            .map(|scope| (*scope).to_owned())
        else {
            return;
        };
        let woke = Woke {
            wake: Some(id),
            ..self.woke_by(session, why, response)
        };
        let message = Message {
            level: Some(zirc_proto::Level::Info),
            ..self.signed(session, woke.event())
        };
        self.publish_message(&scope, message);
    }

    /// What `response`, which woke `session` for `why`, was, as its wake's line says it.
    fn woke_by(&self, session: &str, why: Why, response: &Response) -> Woke {
        match response {
            Response::Delivery(delivery) => self.delivery_woke(why, delivery),
            Response::Direct(direct) => direct_woke(direct),
            Response::Reaction(reaction) => Woke {
                from: Some(reaction.from.clone()),
                line: Some(reaction.to.clone()),
                ..Woke::new(What::Reacted, self.nick_is(&reaction.from, ""))
            },
            Response::Answered(answered) => Woke {
                from: Some(answered.by.clone()),
                line: Some(answered.msgid.clone()),
                ..Woke::new(What::Decided, By::Person)
            },
            Response::Overlap(overlap) => Woke {
                from: Some(overlap.nick.clone()),
                ..Woke::new(What::Overlap, By::Agent)
            },
            Response::Conflict(conflict) => Woke {
                from: Some(conflict.nick.clone()),
                branch: Some(conflict.branch.clone()),
                ..Woke::new(What::Conflict, By::Agent)
            },
            // The other worktree's session, on this machine.
            Response::LocalOverlap(overlap) => Woke {
                from: self.client.as_ref().map(|client| client.nick().to_owned()),
                session: overlap
                    .worktrees
                    .iter()
                    .filter_map(|worktree| self.session_of(worktree))
                    .find(|other| *other != session)
                    .map(str::to_owned),
                ..Woke::new(What::Overlap, By::Agent)
            },
            _ => Woke::new(What::Other, By::Zirc),
        }
    }

    /// Whose `nick` is, `session` being the session its message says: CI's or another
    /// integration's, an agent's on a machine, zircd's own word, or a person's.
    fn nick_is(&self, nick: &str, session: &str) -> By {
        if zirc_proto::names::is_integration_nick(&nick.to_ascii_lowercase()) {
            By::Ci
        } else if session == WATCHER_SESSION {
            By::Zirc
        } else if self.is_machine(nick) {
            By::Agent
        } else {
            By::Person
        }
    }

    /// A line said in a channel, as a wake's line says it.
    fn delivery_woke(&self, why: Why, delivery: &Delivery) -> Woke {
        let message = &delivery.message;
        // A person's words come with no session; a machine's say which of its agents'.
        let by = match self.nick_is(&delivery.from, &message.session) {
            By::Person if delivery.hosted_by.is_some() || !message.session.is_empty() => By::Agent,
            by => by,
        };
        let said = Woke {
            from: (by != By::Ci).then(|| delivery.from.clone()),
            session: (by == By::Agent).then(|| message.session.clone()),
            channel: Some(self.wire_name(&delivery.channel)),
            line: delivery.id.clone(),
            ..Woke::new(What::Other, by)
        };
        if let Event::Custom { kind, data, .. } = &message.event
            && ci::only_cis(kind)
        {
            let data = ci::Data::of(data);
            // A branch that landed changes the session's files: that, not how its build went.
            let what = if why == Why::BranchLanded {
                What::Landed
            } else {
                What::Build
            };
            return Woke {
                what,
                by: By::Ci,
                from: None,
                build: data.build,
                // How it stands as a card's state reads, the name CI gives it.
                state: data.state.and_then(|state| {
                    serde_json::from_value(serde_json::Value::String(state)).ok()
                }),
                branch: data.branch,
                landing: kind == ci::LANDING_KIND,
                ..said
            };
        }
        if let Some(state) = zirc_proto::state::State::of(&message.event) {
            return Woke {
                what: What::State,
                by: By::Ci,
                from: None,
                level: Some(state.level),
                branch: Some(state.name),
                ..said
            };
        }
        let what = match (why, &message.event) {
            (Why::Decision, _) => What::Decided,
            (Why::Mention, _) if delivery.reply.is_some() => What::Replied,
            (Why::Mention, _) => What::Mentioned,
            (Why::Intent, _) => What::Intends,
            (Why::Overlap | Why::Paths, Event::Commit { .. }) => What::Committed,
            (Why::Overlap, _) => What::Overlap,
            (Why::Conflict, _) => What::Conflict,
            (Why::StackedOn, _) => What::StackedOn,
            (Why::Landing | Why::BranchLanded, _) => What::Landed,
            (Why::Paths | Why::Topic | Why::Channel, _) => What::Posted,
            _ => What::Other,
        };
        let branch = match &message.event {
            Event::Push { branch, .. } if what == What::Landed => Some(branch.clone()),
            _ => None,
        };
        Woke {
            what,
            branch,
            ..said
        }
    }
}

/// What was sent to the machine directly, as a wake's line says it.
fn direct_woke(direct: &Direct) -> Woke {
    // What woke a role, as its nick said it.
    if let Some(woke) = &direct.woke {
        return woke.clone();
    }
    let (what, by) = if direct.query.is_some() {
        (What::Asked, By::Agent)
    } else if direct.answer.is_some() {
        (What::Answered, By::Agent)
    } else if direct.note {
        (What::Noted, By::Agent)
    } else {
        (What::Wrote, By::Person)
    };
    Woke {
        from: Some(direct.from.clone()),
        session: direct.from_session.clone(),
        line: direct.id.clone(),
        role: direct.role.clone(),
        ..Woke::new(what, by)
    }
}
