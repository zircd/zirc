//! Sessions their person paused from an app ([`zirc_proto::pause`]): only this machine's
//! owner pauses its sessions, by an `x-zirc/pause` event sent to its nick. The session hears
//! it on its watch, as a direct message with [`Direct::pause`], whose hint says to stop at a
//! safe point and wait. While it's paused, its watch wakes only for the owner's own words:
//! what else concerns it is held, and told once the pause ends, with the owner's resume or
//! their next message to it (the user's choices, 28 Nov 2026). The live list says which
//! sessions are paused, so every app's row says so ([`zirc_proto::session::Live::paused`]).
//!
//! A pause lasts while zircd runs: a restart forgets it, as the session itself still waits
//! for its person's word, which ends a pause anyway.

use std::collections::{BTreeMap, BTreeSet};

use zirc_core::ipc::{Delivery, Direct, Response, Why};
use zirc_proto::{Event, Message, pause};

use super::{Daemon, WATCHER_SESSION, now};

/// The most lines held for a paused session, its newest kept: as many as zircd keeps for
/// a session with no watch open.
const HELD_MOST: usize = super::MISSED;

/// A session paused: since when, and what was held for it meanwhile.
#[derive(Debug, Default)]
pub(super) struct Paused {
    /// When it was paused, in seconds since the Unix epoch.
    pub(super) since: u64,
    /// What concerned it while paused, each as its watch would have heard it.
    held: Vec<Response>,
}

/// Whether `response` is the owner's own word to a session: a pause or resume, or a direct
/// message of theirs. Never another agent's note, query or answer, a colleague's reply, or
/// what came to a role.
fn owners_word(response: &Response) -> bool {
    matches!(response, Response::Direct(direct) if owners(direct))
}

/// Whether `direct` is the owner's own: see [`owners_word`].
fn owners(direct: &Direct) -> bool {
    !direct.note
        && !direct.colleague
        && direct.query.is_none()
        && direct.answer.is_none()
        && direct.role.is_none()
        && direct.woke.is_none()
}

/// The pause or resume `body`, sent to this machine, carries, if it's one.
pub(super) fn pause_of(body: &str) -> Option<pause::Pause> {
    let message = zirc_proto::decode(body).ok()??;
    pause::Pause::of(&message.event)
}

impl Daemon {
    /// The owner's pause or resume `asked` of a session here, as its watchers hear it: `None`
    /// for one that names no session working here, which reaches nobody.
    pub(super) fn paused_by_owner(&self, from: &str, asked: &pause::Pause) -> Option<Direct> {
        let session = self.session_here(&asked.session)?;
        Some(Direct {
            from: from.to_owned(),
            from_session: None,
            session: Some(session),
            text: asked.text(),
            query: None,
            answer: None,
            note: false,
            colleague: false,
            why: None,
            id: None,
            org: None,
            role: None,
            woke: None,
            pause: Some(asked.paused),
        })
    }

    /// Notes what the owner's `direct` does to the pauses: a pause pauses its session; a
    /// resume, or any other message of theirs to a paused session, or to every session here,
    /// ends the pauses it reaches. Returns the sessions whose pause it ended, with what was
    /// held for each, to tell once `direct` is told ([`Daemon::tell_held`]).
    pub(super) fn note_pause(&mut self, direct: &Direct) -> Vec<(String, Vec<Response>)> {
        if !owners(direct) {
            return Vec::new();
        }
        if direct.pause == Some(true) {
            if let Some(session) = &direct.session {
                tracing::info!("{session} paused by its owner");
                self.live.paused.entry(session.clone()).or_insert(Paused {
                    since: now(),
                    held: Vec::new(),
                });
            }
            return Vec::new();
        }
        let ended: Vec<String> = self
            .live
            .paused
            .keys()
            .filter(|paused| direct.session.as_deref().is_none_or(|to| to == *paused))
            .cloned()
            .collect();
        ended
            .into_iter()
            .filter_map(|session| {
                let paused = self.live.paused.remove(&session)?;
                tracing::info!("{session}'s pause ended by its owner");
                Some((session, paused.held))
            })
            .collect()
    }

    /// Holds each wake in `reasons` of a paused session that isn't the owner's own word,
    /// to tell once its pause ends.
    pub(super) fn hold_paused(&mut self, response: &Response, reasons: &mut [Option<Why>]) {
        if self.live.paused.is_empty() || owners_word(response) {
            return;
        }
        let mut held: BTreeSet<String> = BTreeSet::new();
        let sessions: Vec<Option<String>> = self
            .watchers
            .iter()
            .map(|watcher| watcher.session.clone())
            .collect();
        for (session, why) in sessions.iter().zip(reasons.iter_mut()) {
            let (Some(session), Some(reason)) = (session, *why) else {
                continue;
            };
            if !self.live.paused.contains_key(session) {
                continue;
            }
            *why = None;
            if held.insert(session.clone()) {
                let dressed = self.dressed(response, Some(reason), Some(session));
                if let Some(paused) = self.live.paused.get_mut(session) {
                    if paused.held.len() >= HELD_MOST {
                        paused.held.remove(0);
                    }
                    paused.held.push(dressed);
                }
            }
        }
    }

    /// Tells `session`'s watches what was `held` for it while it was paused, after the word
    /// that ended its pause, under a line that says so.
    pub(super) fn tell_held(&mut self, session: &str, held: Vec<Response>) {
        if held.is_empty() {
            return;
        }
        let me = self
            .client
            .as_ref()
            .map(|client| client.nick().to_owned())
            .unwrap_or_default();
        let say = |body: String| {
            Response::Delivery(Delivery {
                why: Some(Why::Mention),
                ..Delivery::new(
                    zirc_core::project::ORG_CHANNEL.to_owned(),
                    me.clone(),
                    Message::new(WATCHER_SESSION, Event::Text { body }),
                )
            })
        };
        let lines = std::iter::once(say(pause::held_heading(session, held.len())))
            .chain(held)
            .chain(std::iter::once(say(pause::held_end(session))));
        let watchers: Vec<_> = self
            .watchers
            .iter()
            .filter(|watcher| watcher.reachable() && watcher.session.as_deref() == Some(session))
            .map(|watcher| watcher.heard.clone())
            .collect();
        // With no watch open, they wait for its next, as what it missed.
        if watchers.is_empty() {
            let at = self.clock.now();
            self.missed
                .entry(session.to_owned())
                .or_default()
                .extend(lines.map(|line| (at, line)));
            return;
        }
        for line in lines {
            for heard in &watchers {
                let _ = heard.try_send(line.clone());
            }
        }
    }

    /// The sessions here paused, each with since when, as the live list says them: those it
    /// still knows.
    pub(super) fn paused_now(&self) -> BTreeMap<String, u64> {
        self.live
            .paused
            .iter()
            .filter(|(session, _)| self.live.knows(session))
            .map(|(session, paused)| (session.clone(), paused.since))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Org, Watcher};
    use super::*;
    use tokio::sync::mpsc;
    use zirc_irc::Line;

    /// A daemon owned by mattias with one watch, of `cc-1`, and what it hears.
    fn daemon(home: &std::path::Path) -> (Daemon, mpsc::Receiver<Response>) {
        let org = Org {
            name: "zirc".into(),
            first: true,
            state: home.to_owned(),
            several: false,
        };
        let (inputs, _received) = mpsc::unbounded_channel();
        let mut daemon = Daemon::new(home.to_owned(), org, "irc.example".into(), None, inputs);
        daemon.owner = Some("mattias".into());
        let (heard, receiver) = mpsc::channel(64);
        daemon.watchers.push(Watcher {
            heard,
            session: Some("cc-1".into()),
        });
        (daemon, receiver)
    }

    /// What `account`, as `nick`, sends this machine: `body`.
    fn sent(daemon: &mut Daemon, nick: &str, account: &str, body: &str) {
        let line: Line = format!("@account={account} :{nick}!u@h PRIVMSG mattias-mbp :{body}")
            .parse()
            .unwrap();
        daemon.heard_direct(&line, nick, body);
    }

    fn pause(paused: bool) -> String {
        let event = pause::Pause {
            session: "cc-1".into(),
            paused,
        }
        .event();
        zirc_proto::encode(&Message::new("", event)).unwrap()
    }

    /// What the watch heard that concerns it, as lines' texts.
    fn woke(hears: &mut mpsc::Receiver<Response>) -> Vec<String> {
        let mut said = Vec::new();
        while let Ok(response) = hears.try_recv() {
            match response {
                Response::Direct(direct) if direct.why.is_some() => {
                    said.push(format!("{:?}: {}", direct.pause, direct.text));
                }
                Response::Delivery(delivery) if delivery.why.is_some() => {
                    said.push(delivery.message.event.human_text());
                }
                _ => {}
            }
        }
        said
    }

    /// Another agent's note for cc-1, which concerns it.
    fn note(daemon: &mut Daemon) {
        daemon.tell_direct(Direct {
            from: "ada-mbp".into(),
            from_session: Some("cc-9".into()),
            session: Some("cc-1".into()),
            text: "I changed login.rs".into(),
            query: None,
            answer: None,
            note: true,
            colleague: false,
            why: None,
            id: None,
            org: None,
            role: None,
            woke: None,
            pause: None,
        });
    }

    #[tokio::test]
    async fn a_paused_session_hears_only_its_owner_and_what_was_held_once_resumed() {
        let home = tempfile::tempdir().unwrap();
        let (mut daemon, mut hears) = daemon(home.path());
        daemon
            .live
            .update(&BTreeSet::from(["cc-1".to_owned()]), now());

        // Someone else's pause reaches nobody.
        sent(&mut daemon, "ada", "ada", &pause(true));
        assert!(woke(&mut hears).is_empty());
        assert!(daemon.paused_now().is_empty());

        sent(&mut daemon, "mattias", "mattias", &pause(true));
        assert_eq!(
            woke(&mut hears),
            ["Some(true): Pause cc-1: stop at a safe point and wait"]
        );
        assert_eq!(
            daemon.paused_now().keys().collect::<Vec<_>>(),
            ["cc-1"],
            "the live list says it's paused"
        );

        // A teammate's note waits.
        note(&mut daemon);
        assert!(woke(&mut hears).is_empty());

        // The owner resumes it: it hears that, then what was held.
        sent(&mut daemon, "mattias", "mattias", &pause(false));
        assert_eq!(
            woke(&mut hears),
            [
                "Some(false): Resume cc-1",
                "cc-1: while you were paused, this concerned you:",
                "None: I changed login.rs",
                "cc-1: that's what came while you were paused",
            ]
        );
        assert!(daemon.paused_now().is_empty());

        // Its owner's message ends a pause too.
        sent(&mut daemon, "mattias", "mattias", &pause(true));
        note(&mut daemon);
        woke(&mut hears);
        sent(
            &mut daemon,
            "mattias",
            "mattias",
            "cc-1: how far did you get?",
        );
        let told = woke(&mut hears);
        assert_eq!(told[0], "None: how far did you get?");
        assert_eq!(told.len(), 4, "{told:?}");
        assert!(daemon.paused_now().is_empty());
    }
}
