//! Reports that can wait: a session's post naming another to say something's done or has
//! landed, with no question in it and nothing gone wrong, such as `cc-1: login landed as
//! 1a2b3c4`. It names the session it's for, but needs nothing of it now, so it doesn't wake
//! that session: it waits, and comes just before the next line that does, with a line saying
//! so. A coordinator was woken 26 times in 40 by such reports, 20 times for nothing it then
//! did (the chat analysis of 5 Nov 2026).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use zirc_core::chat::{SenderKind, sender_kind};
use zirc_core::ipc::{Delivery, Response, Why};
use zirc_core::paths;
use zirc_proto::{Event, Message};

use super::{Daemon, WATCHER_SESSION, read_saved};

/// How many reports wait for each session at most: the oldest go first.
const KEPT: usize = 20;

/// The reports waiting for each session's next wake, oldest first.
pub(super) type Later = BTreeMap<String, VecDeque<Response>>;

/// What each session has lately heard in a digest, by what it said, so the same line said
/// again, as CI says a landing in more than one channel, isn't in the next one too.
pub(super) type Told = BTreeMap<String, VecDeque<String>>;

/// How many lines told in digests are remembered for each session.
const TOLD: usize = 100;

/// What `response` says, as a digest compares it: who said what.
fn said(response: &Response) -> Option<String> {
    let Response::Delivery(delivery) = response else {
        return None;
    };
    Some(format!(
        "{}: {}",
        delivery.from,
        delivery.message.event.human_text()
    ))
}

/// Whether `response` can wait for a session's next wake rather than wake it: a report (see
/// the module's doc); CI saying a session's landing, deploy or Apple apps went as they
/// should, which needs nothing of it (its failures still wake it); or zircd's own notice
/// that a checkout it watched is gone.
pub(super) fn waits(response: &Response) -> bool {
    report(response) || ci_success(response) || housekeeping(response)
}

/// Whether `response` is an integration's line saying something went as it should
/// ([`super::routine`]), such as `cc-1: landed login as 1a2b3c4 (build 7)`.
fn ci_success(response: &Response) -> bool {
    matches!(response, Response::Delivery(delivery)
        if sender_kind(&delivery.from, None) == SenderKind::Integration
            && super::routine(&delivery.from, &delivery.message))
}

/// Whether `response` is zircd telling a session a checkout it was watched in is gone.
fn housekeeping(response: &Response) -> bool {
    matches!(response, Response::Delivery(delivery)
        if delivery.message.session == WATCHER_SESSION
            && matches!(&delivery.message.event, Event::Text { body }
                if body.contains("no longer watched")))
}

/// Whether `response` is a report that can wait (see the module's doc).
fn report(response: &Response) -> bool {
    let Response::Delivery(delivery) = response else {
        return false;
    };
    let Event::Text { body } = &delivery.message.event else {
        return false;
    };
    // A session's post, never a person's or an integration's.
    if delivery.message.session.is_empty()
        || sender_kind(&delivery.from, None) == SenderKind::Integration
    {
        return false;
    }
    says_done(body)
}

/// Whether `text` says only that something is done, landed or out: no question, nothing
/// that failed, broke or waits on someone.
fn says_done(text: &str) -> bool {
    let lower = text.to_lowercase();
    if lower.contains('?') {
        return false;
    }
    let needs = [
        "fail",
        "broke",
        "broken",
        "conflict",
        "error",
        "blocked",
        "can't",
        "couldn't",
        "cannot",
        "need",
        "approve",
        "please",
        "waiting on",
        "stuck",
    ];
    if needs.iter().any(|word| lower.contains(word)) {
        return false;
    }
    if zirc_proto::says_done(&lower) {
        return true;
    }
    // After an address such as `cc-1:`, as reports start.
    let said = zirc_proto::said(&lower);
    // CI's line for a session's own landing, `cc-1: landed login as …`, is its news.
    if said.starts_with("landed ") {
        return false;
    }
    zirc_proto::says_done(said)
        || said.starts_with("done ")
        || [" landed ", " is out", " deployed ", " published "]
            .iter()
            .any(|word| format!(" {said} ").contains(word))
}

/// Whether `response` is an agent session saying something, which in a topic a session
/// joined can wait for its next wake: 11 of a day's topic wakes on one machine, every one of
/// them another session's post, led to nothing (the chat analysis, 11 Nov 2026). It carries
/// a session that isn't zircd's own, nor a person's at a terminal, whose session is their
/// own name; CI's failures, alerts and people in the topic still wake it.
pub(super) fn agent_chatter(response: &Response) -> bool {
    let Response::Delivery(delivery) = response else {
        return false;
    };
    let session = delivery.message.session.as_str();
    let from = delivery.from.as_str();
    !session.is_empty()
        && session != WATCHER_SESSION
        && !session.eq_ignore_ascii_case(from)
        && !session.eq_ignore_ascii_case(delivery.hosted_by.as_deref().unwrap_or(from))
        && sender_kind(from, None) != SenderKind::Integration
}

/// Whether `response` is an agent session's broadcast, `zirc post --everyone`: it reaches
/// every session, with its next wake rather than as one (the user's choice, 24 Nov 2026:
/// "Nobody; next wake"). One such post woke 15 sessions at once, and 6 of a day's 19 such
/// wakes led to anything. A person's own `@everyone` still wakes them all.
fn agent_broadcast(response: &Response) -> bool {
    matches!(response, Response::Delivery(delivery) if delivery.message.everyone)
        && agent_chatter(response)
}

impl Daemon {
    /// Whether `response` concerns `session` only as an agent's broadcast
    /// ([`agent_broadcast`]), which waits for its next wake: not one that names it, or a
    /// branch it's built on, which wakes it as any post that does.
    pub(super) fn broadcast_waits(&self, response: &Response, session: &str) -> bool {
        let Response::Delivery(delivery) = response else {
            return false;
        };
        let Event::Text { body } = &delivery.message.event else {
            return false;
        };
        agent_broadcast(response) && !self.named(body, session)
    }

    /// Keeps `response` for each watching session it would only wake as a report that names
    /// it, which then hears it now without a reason, in `reasons`.
    /// So too what another agent session says in a topic a session joined, and an agent's
    /// broadcast ([`Self::broadcast_waits`]).
    pub(super) fn hold_reports(&mut self, response: &Response, reasons: &mut [Option<Why>]) {
        let report = waits(response);
        let chatter = agent_chatter(response);
        let broadcast: BTreeSet<String> = self
            .watchers
            .iter()
            .filter_map(|watcher| watcher.session.clone())
            .filter(|session| self.broadcast_waits(response, session))
            .collect();
        if !report && !chatter && broadcast.is_empty() {
            return;
        }
        let waiting: Vec<(String, Why)> = self
            .watchers
            .iter()
            .zip(reasons.iter_mut())
            .filter_map(|(watcher, why)| {
                let session = watcher.session.clone()?;
                let held = match *why {
                    Some(Why::Mention) if report || broadcast.contains(&session) => Why::Mention,
                    Some(Why::Topic) if chatter => Why::Topic,
                    _ => return None,
                };
                *why = None;
                Some((session, held))
            })
            .collect();
        for (session, why) in waiting {
            self.keep_for_later(&session, response, why);
        }
    }

    /// For each session a watch of which `reasons` wakes, what waited for it, to send first:
    /// no longer kept.
    pub(super) fn waited_for(
        &mut self,
        reasons: &[Option<Why>],
    ) -> BTreeMap<String, Vec<Response>> {
        let woken: BTreeSet<String> = self
            .watchers
            .iter()
            .zip(reasons)
            .filter_map(|(watcher, why)| why.and(watcher.session.clone()))
            .filter(|session| self.later.contains_key(session))
            .collect();
        woken
            .into_iter()
            .map(|session| {
                let earlier = self.take_later(&session);
                (session, earlier)
            })
            .collect()
    }

    /// Remembers that `session` heard `response`, woken by it or in a digest, so no later
    /// digest says it again.
    pub(super) fn heard_now(&mut self, session: &str, response: &Response) {
        let Some(line) = said(response) else { return };
        let told = self.later_told.entry(session.to_owned()).or_default();
        if !told.contains(&line) {
            told.push_back(line);
        }
        while told.len() > TOLD {
            told.pop_front();
        }
    }

    /// Keeps `response`, a report naming `session`, until its next wake.
    pub(super) fn keep_for_later(&mut self, session: &str, response: &Response, why: Why) {
        let line = said(response);
        let told = self.later_told.get(session);
        let kept = self.later.entry(session.to_owned()).or_default();
        let response = super::with_why(response.clone(), Some(why));
        let again = line.as_ref().is_some_and(|line| {
            told.is_some_and(|told| told.contains(line))
                || kept.iter().any(|it| said(it).as_ref() == Some(line))
        });
        if again || kept.contains(&response) {
            return;
        }
        kept.push_back(response);
        while kept.len() > KEPT {
            kept.pop_front();
        }
        self.save_later();
    }

    /// Saves what waits for each session's next wake, for after a restart, which replays
    /// only what was said after what zircd last heard: in a file only this machine's user
    /// reads, as it holds what was said to them.
    pub(super) fn save_later(&self) {
        let json = serde_json::to_string(&self.later).expect("what waits serializes");
        if let Err(err) = paths::write_private(&paths::later(&self.org.state), &json) {
            tracing::warn!("couldn't save what waits for sessions' next wakes: {err}");
        }
    }

    /// Takes in what waited for each session's next wake when zircd last stopped.
    pub(super) fn restore_later(&mut self) {
        if let Some(saved) = read_saved::<Later>(&paths::later(&self.org.state)) {
            self.later = saved;
        }
    }

    /// The reports kept for `session`, said as a watch line introduces them, to send just
    /// before a line that wakes it; nothing when none are kept. They're no longer kept.
    pub(super) fn take_later(&mut self, session: &str) -> Vec<Response> {
        let kept: Vec<Response> = self.later.remove(session).unwrap_or_default().into();
        if !kept.is_empty() {
            self.save_later();
        }
        for response in &kept {
            self.heard_now(session, response);
        }
        // Held by the breaker, another agent's queries have no channel to introduce them in:
        // they're said as they came.
        let Some(channel) = kept.iter().find_map(super::channel_of).map(str::to_owned) else {
            return kept;
        };
        let me = self
            .client
            .as_ref()
            .map(|client| client.nick().to_owned())
            .unwrap_or_default();
        let intro = Response::Delivery(Delivery {
            why: Some(Why::Mention),
            ..Delivery::new(
                channel,
                me,
                Message::new(
                    WATCHER_SESSION,
                    Event::Text {
                        body: format!("{session}: meanwhile, for you and needing nothing yet:"),
                    },
                ),
            )
        });
        std::iter::once(intro).chain(kept).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_says_only_that_something_is_done() {
        for text in [
            "cc-1: chat analysis 1 of 5 landed as 7790af3 (build 1204)",
            "cc-1: done: the login page",
            "done: split the daemon",
            "cc-272fe8: zirc 0.1.24 is out",
        ] {
            assert!(says_done(text), "{text}");
        }
        for text in [
            "cc-1: build 12 failed in test",
            "cc-1: landed, but master broke: can you look?",
            "cc-1: landed login; I need your OK to deploy",
            "cc-1: are you in daemon.rs?",
            "cc-1: taking: the release",
            "cc-1: about to change ops/land.sh",
            "cc-1: landed login as 1a2b3c (build 7)",
        ] {
            assert!(!says_done(text), "{text}");
        }
    }
}
