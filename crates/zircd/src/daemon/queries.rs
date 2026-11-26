//! Direct messages, notes and queries between sessions, people and machines.

use std::collections::BTreeSet;

use anyhow::Context as _;
use anyhow::Result;
use zirc_core::agents;
use zirc_core::agents::{Queries, Sender, Standing};
use zirc_core::ipc::{Direct, Response};
use zirc_proto::Event;

use super::{Asked, Daemon, Queried, WROTE_FOR, is_nick, now, query_id, remember};

/// A query's id, the session it's for, if it names one, and what it asks.
struct Asking {
    id: String,
    to: Option<String>,
    text: String,
}

/// The session a refusal comes from: the machine's zircd, not any agent on it.
const REFUSER: &str = "zircd";

impl Daemon {
    /// A message to this machine's nick, as its watchers hear it, if they're to: only this
    /// machine's owner reaches its agents, as the server says who sent it
    /// ([`agents::standing`]), never by nick: from their own account, or their own client
    /// under another name. One of the owner's machines is not the owner, whatever it sends:
    /// its agents share its account, so a line from it is an agent's, which `zirc ask` is
    /// for. Everything else is filtered out too: anyone who can message the machine could
    /// otherwise steer its agents.
    ///
    /// Any line will do, with or without the `/agent` older apps start it with. Its first
    /// word may name one of the sessions working here, as `cc-1531f8` or `cc-1531f8:`, for
    /// that session alone; one that names a session not here comes back as that session and
    /// the notice to answer it with, `Err`, rather than reaching every session.
    pub(super) fn direct(
        &self,
        sender: Sender<'_>,
        from: &str,
        body: &str,
    ) -> Option<Result<Direct, (String, String)>> {
        let owner = self.owner.as_deref()?;
        let standing = agents::standing(sender, Some(owner), self.zirc_dev);
        if standing != Standing::Owner {
            return self.colleague_replied(standing, sender, from, body).map(Ok);
        }
        let (text, from_session) = match zirc_proto::decode(body) {
            Ok(Some(message)) => match message.event {
                Event::Text { body } => (body, Some(message.session)),
                _ => return None,
            },
            Ok(None) if !body.starts_with('\x01') => (body.to_owned(), None),
            _ => return None,
        };
        let text = without_agent(&text);
        let (first, more) = text
            .split_once(char::is_whitespace)
            .map_or((text, ""), |(first, more)| (first, more.trim_start()));
        let name = first.strip_suffix([':', ',']).unwrap_or(first);
        let (session, text) = match self.session_here(name) {
            Some(session) => (Some(session), more),
            None if looks_like_session(name) => {
                return Some(Err((name.to_owned(), self.no_session(name))));
            }
            None => (None, text),
        };
        if text.is_empty() {
            return None;
        }
        Some(Ok(Direct {
            from: from.to_owned(),
            from_session,
            session,
            text: text.to_owned(),
            query: None,
            answer: None,
            note: false,
            colleague: false,
            why: None,
            id: None,
            org: None,
            role: None,
            woke: None,
        }))
    }

    /// A reply from a member that one of this machine's sessions wrote to lately (`zirc dm
    /// --to`), for that session, while the organisation lets colleagues answer agents: a
    /// colleague's word, never the person's. Anyone else's message, or one to a session that
    /// didn't write to them, is dropped.
    fn colleague_replied(
        &self,
        standing: Standing,
        sender: Sender<'_>,
        from: &str,
        body: &str,
    ) -> Option<Direct> {
        if standing != Standing::Member || !self.colleagues().answers {
            return None;
        }
        let them = sender.account().unwrap_or(from);
        let since = now().saturating_sub(WROTE_FOR);
        let wrote = self
            .wrote_to
            .iter()
            .rev()
            .find(|wrote| wrote.at >= since && wrote.to.eq_ignore_ascii_case(them))?;
        let (text, from_session) = match zirc_proto::decode(body) {
            Ok(Some(message)) => match message.event {
                Event::Text { body } => (body, Some(message.session)),
                _ => return None,
            },
            Ok(None) if !body.starts_with('\x01') => (body.to_owned(), None),
            _ => return None,
        };
        if text.trim().is_empty() {
            return None;
        }
        Some(Direct {
            from: from.to_owned(),
            from_session,
            session: Some(wrote.session.clone()),
            text,
            query: None,
            answer: None,
            note: false,
            colleague: true,
            why: None,
            id: None,
            org: None,
            role: None,
            woke: None,
        })
    }

    /// Answers the query `id` from `asker`, a session on the machine `from`, with `why` it
    /// won't be answered, so it doesn't wait. One that can't be sent is only logged.
    fn refuse(&self, from: &str, asker: &str, id: &str, why: &str) {
        let Ok(client) = self.connected() else {
            return;
        };
        let event = Event::Answer {
            query: id.to_owned(),
            to: Some(asker.to_owned()),
            text: why.to_owned(),
        };
        if let Err(e) = zirc_core::send::send_message(client, from, &self.signed(REFUSER, event)) {
            tracing::warn!("couldn't tell {from} its query was refused: {e}");
        }
    }

    /// What to tell the owner when they name `name`, which isn't a session working here.
    fn no_session(&self, name: &str) -> String {
        let mut here: Vec<&str> = self
            .repos
            .values()
            .flat_map(|watched| watched.sessions.names())
            .chain(self.watchers.iter().filter_map(|w| w.session.as_deref()))
            .collect();
        here.sort_unstable();
        here.dedup();
        if here.is_empty() {
            format!("No session {name} here, nor any other: this didn't reach anyone.")
        } else {
            format!(
                "No session {name} here: this didn't reach anyone. Working here: {}.",
                here.join(", ")
            )
        }
    }

    /// The session working here called `name`, if one is.
    pub(super) fn session_here(&self, name: &str) -> Option<String> {
        self.repos
            .values()
            .flat_map(|watched| watched.sessions.names())
            .chain(self.watchers.iter().filter_map(|w| w.session.as_deref()))
            .find(|session| *session == name)
            .map(str::to_owned)
    }

    /// A query from `asker`, a session on the machine `from` of `standing`, as its watchers
    /// hear it, if they're to; one this machine won't take is refused with an answer saying
    /// why, to a colleague's agent, so it doesn't wait ([`Daemon::refuse`]).
    fn queried(
        &mut self,
        standing: Standing,
        from: &str,
        account: Option<&str>,
        asker: String,
        asking: Asking,
    ) -> Option<Direct> {
        let Asking { id, to, text } = asking;
        let colleagues_may = self.colleagues().queries;
        if !agents::may_query(self.agents.queries.under(colleagues_may), standing) {
            tracing::info!("dropped a query from {from} ({account:?}): not let in");
            // A colleague's agent is told, so it doesn't wait; anyone else isn't.
            if standing == Standing::Member {
                let why = agents::refused(self.agents.queries, colleagues_may);
                self.refuse(from, &asker, &id, &why);
            }
            return None;
        }
        // Only for the session it names: never every session's when that one isn't here.
        let session = match to {
            Some(to) => {
                let Some(session) = self.session_here(&to) else {
                    tracing::info!("dropped a query from {from} for {to}: no such session here");
                    self.refuse(from, &asker, &id, &agents::no_such_session(&to));
                    return None;
                };
                Some(session)
            }
            None => None,
        };
        remember(
            &mut self.queried,
            Queried {
                id: id.clone(),
                from: from.to_owned(),
                session: asker.clone(),
            },
        );
        Some(Direct {
            from: from.to_owned(),
            from_session: Some(asker),
            session,
            text,
            query: Some(id),
            answer: None,
            note: false,
            colleague: false,
            why: None,
            id: None,
            org: None,
            role: None,
            woke: None,
        })
    }

    /// A query or an answer sent to this machine, as its watchers hear it, if they're to.
    /// A query reaches them only when this machine lets its sender query its agents, told
    /// by the account the server says sent it ([`agents::may_query`]); it's for the session
    /// it names, and for no one when that one doesn't work here, or for every session when
    /// it names none. An answer reaches the
    /// session that asked, and only for a query asked of the machine that answers.
    pub(super) fn queried_or_answered(
        &mut self,
        sender: Sender<'_>,
        from: &str,
        body: &str,
    ) -> Option<Direct> {
        let account = sender.account();
        let message = zirc_proto::decode(body).ok()??;
        match message.event {
            Event::Query { id, to, text } => {
                let standing = agents::standing(sender, self.owner.as_deref(), self.zirc_dev);
                self.queried(
                    standing,
                    from,
                    account,
                    message.session,
                    Asking { id, to, text },
                )
            }
            Event::Note { to, text } => {
                let standing = agents::standing(sender, self.owner.as_deref(), self.zirc_dev);
                let queries = self.agents.queries.under(self.colleagues().queries);
                if !agents::may_query(queries, standing) {
                    tracing::info!("dropped a note from {from} ({account:?}): not let in");
                    return None;
                }
                // Only for the session it names: never every session's.
                let Some(session) = self.local_session(&to) else {
                    tracing::info!("dropped a note from {from} for {to}: no such session here");
                    return None;
                };
                Some(Direct {
                    from: from.to_owned(),
                    from_session: Some(message.session),
                    session: Some(session),
                    text,
                    query: None,
                    answer: None,
                    note: true,
                    colleague: false,
                    why: None,
                    id: None,
                    org: None,
                    role: None,
                    woke: None,
                })
            }
            // Only from the machine asked, by the account the server says sent it: on a server
            // that lets people connect without an account, anyone could take a nick that
            // isn't someone's.
            Event::Answer { query, text, .. }
                if account.is_some_and(|account| account.eq_ignore_ascii_case(from)) =>
            {
                let asked = self.asked.iter().find(|asked| {
                    asked.id == query
                        && asked
                            .machines
                            .iter()
                            .any(|machine| machine.eq_ignore_ascii_case(from))
                })?;
                Some(Direct {
                    from: from.to_owned(),
                    from_session: Some(message.session),
                    session: Some(asked.session.clone()),
                    text,
                    query: None,
                    answer: Some(query),
                    note: false,
                    colleague: false,
                    why: None,
                    id: None,
                    org: None,
                    role: None,
                    woke: None,
                })
            }
            _ => None,
        }
    }

    /// Asks `to`'s agents `text`, as `session`: a machine's (`<machine>`), one of its
    /// sessions' (`<machine>/<session>`, or the session alone, [`Daemon::session_alone`]),
    /// or those on each of a person's machines that are around (`<person>`). A query for
    /// this machine's own sessions goes to them here. Returns its id and the machines it
    /// went to.
    pub(super) fn ask(
        &mut self,
        session: &str,
        to: &str,
        text: &str,
    ) -> Result<(String, Vec<String>)> {
        let alone = self.session_alone(to)?;
        let to = alone.as_deref().unwrap_or(to);
        let (nick, target) = match to.split_once('/') {
            Some((nick, target)) => (nick, Some(target.to_owned())),
            None => (to, None),
        };
        anyhow::ensure!(
            is_nick(nick) && target.as_deref().is_none_or(|t| !t.is_empty()),
            "{to:?} isn't a session, <machine>, <machine>/<session> or a person's nick"
        );
        let me = self.client.as_ref().map(|client| client.nick().to_owned());
        let is_me = |name: &str| {
            me.as_deref()
                .is_some_and(|me| me.eq_ignore_ascii_case(name))
        };
        // zirc.dev names machines <handle>-<machine>, and gives handles no dashes.
        let machines: Vec<String> = if nick.contains('-') || is_me(nick) {
            vec![nick.to_owned()]
        } else {
            anyhow::ensure!(
                target.is_none(),
                "{nick} is a person: name one of their machines, as {nick}-<machine>/<session>"
            );
            let mut machines = self.machines_of(nick);
            if self
                .owner
                .as_deref()
                .is_some_and(|owner| owner.eq_ignore_ascii_case(nick))
                && let Some(me) = &me
            {
                machines.push(me.clone());
            }
            anyhow::ensure!(
                !machines.is_empty(),
                "none of {nick}'s machines is around: name one, as {nick}-<machine>"
            );
            machines
        };
        self.queries_asked += 1;
        let id = query_id(self.queries_asked);
        for machine in &machines {
            if is_me(machine) {
                self.ask_here(session, &id, target.as_deref(), text)?;
                continue;
            }
            let client = self.connected()?;
            let event = Event::Query {
                id: id.clone(),
                to: target.clone(),
                text: text.to_owned(),
            };
            zirc_core::send::send_message(client, machine, &self.signed(session, event))
                .with_context(|| format!("couldn't send to {machine}"))?;
        }
        remember(
            &mut self.asked,
            Asked {
                id: id.clone(),
                session: session.to_owned(),
                machines: machines.clone(),
            },
        );
        Ok((id, machines))
    }

    /// `<machine>/<session>` for the one session `to` names alone, by its id or the name it
    /// goes by, as a note's is found ([`zirc_core::notes::resolve`]): until 26 Nov 2026 a
    /// query wanted its machine too, where a note and a settle didn't (a reviewer's weekend
    /// with 14 agents: "some commands wanted fully qualified agent names rather than the
    /// short ones you'd naturally type"). None when `to` says the machine, is a machine's
    /// or a person's nick, or is no session known here, which is taken as it was.
    ///
    /// # Errors
    ///
    /// When several sessions go by it.
    fn session_alone(&self, to: &str) -> Result<Option<String>> {
        let folded = to.to_ascii_lowercase();
        let nick = self
            .client
            .as_ref()
            .is_some_and(|client| client.nick().eq_ignore_ascii_case(to))
            || self
                .owner
                .as_deref()
                .is_some_and(|owner| owner.eq_ignore_ascii_case(to))
            || self.accounts.contains_key(&folded)
            || self.machines.contains_key(&folded);
        if nick || to.contains('/') {
            return Ok(None);
        }
        match zirc_core::notes::resolve(to, &self.known_sessions()) {
            Ok(address) => Ok(Some(address.to_string())),
            Err(several @ zirc_core::notes::Error::Ambiguous(..)) => Err(several.into()),
            Err(_) => Ok(None),
        }
    }

    /// Leaves `text` for the session `to` names, as `session`: by its id or its name, or as
    /// `<machine>/<session>`, wherever it works ([`zirc_core::notes::resolve`]). Returns where
    /// it went, `<machine>/<session>`.
    pub(super) fn note(&mut self, session: &str, to: &str, text: &str) -> Result<String> {
        let address = zirc_core::notes::resolve(to, &self.known_sessions())?;
        self.note_to(session, &address, text)
    }

    /// The session a post saying `text` is addressed to, if it's one agent session
    /// ([`zirc_core::notes::addressed`]), and what it says without the address.
    pub(super) fn note_address(&self, text: &str) -> Option<(zirc_core::notes::Address, String)> {
        let (people, _) = self.known_names();
        let people: Vec<&str> = people.iter().map(String::as_str).collect();
        zirc_core::notes::addressed(text, &people, &self.known_sessions())
            .map(|(address, said)| (address, said.to_owned()))
    }

    /// Leaves `text` for the session at `address`, as `session`: to its machine's nick, or
    /// here when it's this machine's. Returns where it went.
    pub(super) fn note_to(
        &mut self,
        session: &str,
        address: &zirc_core::notes::Address,
        text: &str,
    ) -> Result<String> {
        let text = text.trim();
        anyhow::ensure!(!text.is_empty(), "a note needs something to say");
        let machine = address.machine.as_str();
        anyhow::ensure!(is_nick(machine), "{machine:?} isn't a machine's nick");
        let here = self
            .client
            .as_ref()
            .is_some_and(|client| client.nick().eq_ignore_ascii_case(machine));
        if here {
            let to = self
                .local_session(&address.session)
                .with_context(|| format!("no session called {} works here", address.session))?;
            self.note_here(session, &to, text)?;
            return Ok(format!("{machine}/{to}"));
        }
        let client = self.connected()?;
        let event = Event::Note {
            to: address.session.clone(),
            text: text.to_owned(),
        };
        zirc_core::send::send_message(client, machine, &self.signed(session, event))
            .with_context(|| format!("couldn't send to {machine}"))?;
        Ok(address.to_string())
    }

    /// A note from `session` for `to`, one of this machine's own sessions, told to its watch
    /// unless queries are off.
    pub(super) fn note_here(&mut self, session: &str, to: &str, text: &str) -> Result<()> {
        anyhow::ensure!(
            self.agents.queries != Queries::Off,
            "notes to this machine's agents are off: zirc agents queries mine"
        );
        let me = self
            .client
            .as_ref()
            .map_or_else(String::new, |c| c.nick().to_owned());
        self.tell_direct(Direct {
            from: me,
            from_session: Some(session.to_owned()),
            session: Some(to.to_owned()),
            text: text.to_owned(),
            query: None,
            answer: None,
            note: true,
            colleague: false,
            why: None,
            id: None,
            org: None,
            role: None,
            woke: None,
        });
        Ok(())
    }

    /// A query from `session` for this machine's own sessions, told to their watches here
    /// unless queries are off: to the session `to` names, refused when that one doesn't work
    /// here, or to every session but the one asking.
    pub(super) fn ask_here(
        &mut self,
        session: &str,
        id: &str,
        to: Option<&str>,
        text: &str,
    ) -> Result<()> {
        anyhow::ensure!(
            self.agents.queries != Queries::Off,
            "queries to this machine's agents are off: zirc agents queries mine"
        );
        let to = match to {
            Some(to) => Some(
                self.session_here(to)
                    .ok_or_else(|| anyhow::anyhow!(self.no_session(to)))?,
            ),
            None => None,
        };
        let me = self
            .client
            .as_ref()
            .map_or_else(String::new, |c| c.nick().to_owned());
        remember(
            &mut self.queried,
            Queried {
                id: id.to_owned(),
                from: me.clone(),
                session: session.to_owned(),
            },
        );
        self.tell_direct(Direct {
            from: me,
            from_session: Some(session.to_owned()),
            session: to,
            text: text.to_owned(),
            query: Some(id.to_owned()),
            answer: None,
            note: false,
            colleague: false,
            why: None,
            id: None,
            org: None,
            role: None,
            woke: None,
        });
        Ok(())
    }

    /// Answers the query `id`, heard lately, with `text`, as `session`: to the machine that
    /// asked, or here to the session that did. Returns the machine.
    pub(super) fn reply(&mut self, session: &str, id: &str, text: &str) -> Result<String> {
        let queried = self
            .queried
            .iter()
            .rev()
            .find(|queried| queried.id == id)
            .with_context(|| format!("no query {id} was heard here lately"))?;
        let (from, asker) = (queried.from.clone(), queried.session.clone());
        let me = self.client.as_ref().map(|client| client.nick().to_owned());
        if me
            .as_deref()
            .is_some_and(|me| me.eq_ignore_ascii_case(&from))
        {
            self.tell_watchers(&Response::Direct(Direct {
                from: from.clone(),
                from_session: Some(session.to_owned()),
                session: Some(asker),
                text: text.to_owned(),
                query: None,
                answer: Some(id.to_owned()),
                note: false,
                colleague: false,
                why: None,
                id: None,
                org: None,
                role: None,
                woke: None,
            }));
            return Ok(from);
        }
        let client = self.connected()?;
        // A long answer in parts, each whole, as zirc-remote sends them.
        for part in zirc_core::send::in_parts(text, zirc_core::send::PART_BYTES) {
            let event = Event::Answer {
                query: id.to_owned(),
                to: Some(asker.clone()),
                text: part,
            };
            zirc_core::send::send_message(client, &from, &self.signed(session, event))
                .with_context(|| format!("couldn't send to {from}"))?;
        }
        Ok(from)
    }

    /// The machines of `person` this machine knows are around, from what they've said in
    /// its channels: those named `<person>-<machine>` and logged in as that account, as the
    /// server last said, sorted. A nick without its account could be anyone's.
    pub(super) fn machines_of(&self, person: &str) -> Vec<String> {
        let prefix = format!("{}-", person.to_ascii_lowercase());
        let now = now();
        let mut machines: BTreeSet<String> = BTreeSet::new();
        for channel in &self.joined {
            let nicks = self
                .presence
                .members(channel)
                .into_iter()
                .map(|member| member.nick)
                .chain(self.presence.claims(channel).into_iter().map(|c| c.nick))
                .chain(
                    self.presence
                        .intents(channel, now)
                        .into_iter()
                        .map(|intent| intent.nick),
                );
            machines.extend(nicks.filter(|nick| {
                let folded = nick.to_ascii_lowercase();
                folded.starts_with(&prefix)
                    && self
                        .accounts
                        .get(&folded)
                        .is_some_and(|account| account.eq_ignore_ascii_case(nick))
            }));
        }
        machines.into_iter().collect()
    }
}

/// `text` without the `/agent` older apps start a line to agents with, trimmed.
fn without_agent(text: &str) -> &str {
    match text.trim().strip_prefix("/agent") {
        Some(rest) if rest.is_empty() || rest.starts_with(char::is_whitespace) => rest.trim(),
        _ => text.trim(),
    }
}

/// Whether `word` is shaped like a session's id, `cc-` and hex digits, as Claude Code's
/// are: one that names no session here is a mistake to say, not a line for everyone.
fn looks_like_session(word: &str) -> bool {
    word.strip_prefix("cc-")
        .is_some_and(|id| id.len() >= 4 && id.chars().all(|c| c.is_ascii_hexdigit()))
}
