//! `#ops`'s newest messages, kept as they're heard. `zirc state` and `zirc status` read the
//! states said in its newest [`states::LOOK_BACK`] each time they run; from the server that
//! took 200 to 370 ms, on 10 Nov 2026, against under 10 ms for a command zircd answers itself.
//! So zircd reads them once, as it joins `#ops`, keeps each message heard there after, and
//! answers such reads from what it kept. Those that come while that first read is out wait
//! for it, rather than ask the server again.
//!
//! What's kept is dropped, to be read again, whenever it might not be what the server holds:
//! the connection dropping, a message trimmed from `#ops`, or a server that doesn't echo this
//! machine's own messages, which would leave them out.
//!
//! The first read goes back [`STATES_BACK`], not just the newest [`states::LOOK_BACK`]: a
//! state said before the newest 1000 messages, such as a hold or a break nobody touched for a
//! fortnight, would read as never said. So the newest state of each name said before the
//! messages kept is kept too, and a read of the newest [`states::LOOK_BACK`] or more, as reading
//! states is, has those ahead of them, oldest first, as history comes.

use std::collections::{BTreeMap, VecDeque};

use tokio::sync::oneshot;
use zirc_core::ipc::{Delivery, Request, Response};
use zirc_core::{history, states};
use zirc_irc::Line;
use zirc_proto::colleagues::Colleagues;

use super::Daemon;
use crate::clock::unix;

/// How far back the first read of `#ops` goes for states: 30 days.
pub(super) const STATES_BACK: u64 = 30 * 86_400;

/// The most messages that first read takes: `#ops` had about 1300 in 10 days on 14 Nov 2026.
const READ_BACK: usize = 20_000;

/// The newest messages of `#ops`, or those heard while they're being read.
#[derive(Debug, Default)]
pub(super) struct Kept {
    /// The most kept: the read's limit.
    limit: usize,
    /// The oldest moment the first read asks for, in seconds since the Unix epoch.
    since: u64,
    /// Oldest first, as history comes.
    messages: VecDeque<Delivery>,
    /// The newest state of each (repository, name) said before `messages`.
    before: BTreeMap<(String, String), Delivery>,
    /// Whether the read is in: until then, `messages` holds only those heard meanwhile.
    read: bool,
    /// Reads waiting for it, each with its limit.
    waiting: Vec<(usize, oneshot::Sender<Response>)>,
    /// What members' agents may do with other members and theirs, as zirc's own account
    /// last said here: kept however many messages come after it.
    colleagues: Option<Colleagues>,
}

impl Kept {
    /// About to read back to `since`, keeping the newest `limit`: what's heard meanwhile is
    /// kept, to add to them.
    pub(super) fn reading(limit: usize, since: u64) -> Self {
        Self {
            limit,
            since,
            ..Self::default()
        }
    }

    /// The read that fills it: `#ops` back to its `since`.
    pub(super) fn filling(&self) -> Request {
        Request::Read {
            target: states::CHANNEL.to_owned(),
            limit: READ_BACK,
            since: Some(self.since),
            until: None,
        }
    }

    /// The read is in: `history`, then each heard meanwhile that it doesn't already have.
    /// Those waiting for it are answered.
    pub(super) fn read(&mut self, history: Vec<Delivery>) {
        for delivery in &history {
            self.note_colleagues(delivery);
        }
        let heard = std::mem::replace(&mut self.messages, history.into());
        self.read = true;
        self.trim();
        for delivery in heard {
            self.heard(delivery);
        }
        for (limit, answer) in std::mem::take(&mut self.waiting) {
            if let Some(messages) = self.newest(limit) {
                let _ = answer.send(Response::Messages { messages });
            }
        }
    }

    /// A message heard in `#ops`: kept, unless it already is, as one heard twice.
    pub(super) fn heard(&mut self, delivery: Delivery) {
        let known = delivery.id.is_some()
            && self
                .messages
                .iter()
                .rev()
                .any(|kept| kept.id == delivery.id);
        if known {
            return;
        }
        self.note_colleagues(&delivery);
        self.messages.push_back(delivery);
        if self.read {
            self.trim();
        }
    }

    /// What members' agents may do with other members and theirs, as the organisation last
    /// said here, if it has.
    pub(super) fn said_colleagues(&self) -> Option<Colleagues> {
        self.colleagues
    }

    /// Takes what `delivery` says members' agents may do, if it says it and it's zirc's own
    /// word: from the account no person or machine holds, which the server says sent it.
    fn note_colleagues(&mut self, delivery: &Delivery) {
        let zircs = delivery
            .account
            .as_deref()
            .is_some_and(|account| account.eq_ignore_ascii_case(zirc_proto::names::SYSTEM));
        if zircs && let Some(colleagues) = Colleagues::of(&delivery.message.event) {
            self.colleagues = Some(colleagues);
        }
    }

    /// Down to the newest `limit` messages, keeping the newest state of each name among
    /// those dropped.
    fn trim(&mut self) {
        while self.messages.len() > self.limit {
            let Some(dropped) = self.messages.pop_front() else {
                return;
            };
            if let Some(state) = zirc_proto::state::State::of(&dropped.message.event) {
                self.before.insert((state.repo, state.name), dropped);
            }
        }
    }

    /// The newest `limit` messages, if they're all here: for `limit` of
    /// [`states::LOOK_BACK`] or more, after the newest state of each name said before them.
    pub(super) fn newest(&self, limit: usize) -> Option<Vec<Delivery>> {
        (self.read && limit <= self.limit).then(|| {
            let skip = self.messages.len().saturating_sub(limit);
            let mut before: Vec<Delivery> = if limit >= states::LOOK_BACK {
                self.before.values().cloned().collect()
            } else {
                Vec::new()
            };
            // RFC 3339 in UTC, as the server stamps them, sorts as text.
            before.sort_by(|a, b| a.time.cmp(&b.time));
            before
                .into_iter()
                .chain(self.messages.iter().skip(skip).cloned())
                .collect()
        })
    }
}

/// Whether `read` asks for the newest messages of `#ops`, as reading states does, and how
/// many.
fn newest_of_ops(read: &Request) -> Option<usize> {
    match read {
        Request::Read {
            target,
            limit,
            since: None,
            until: None,
        } if target.eq_ignore_ascii_case(states::CHANNEL) => Some((*limit).max(1)),
        _ => None,
    }
}

/// A read of `#ops`'s newest `limit`, from the server.
fn newest(limit: usize) -> Request {
    Request::Read {
        target: states::CHANNEL.to_owned(),
        limit,
        since: None,
        until: None,
    }
}

impl Daemon {
    /// What members' agents may do with other members and theirs, as the organisation last
    /// said in `#ops`: both allowed when it never has.
    pub(super) fn colleagues(&self) -> Colleagues {
        self.ops
            .as_ref()
            .and_then(Kept::said_colleagues)
            .or(self.colleagues_said)
            .unwrap_or_default()
    }

    /// Answers `read` from what's kept of `#ops`, or has it wait for the read that keeps it.
    /// Otherwise gives `answer` back, for the server to answer: a read of `#ops`'s newest
    /// then starts keeping them, unless one is already out.
    pub(super) fn read_kept(
        &mut self,
        read: &Request,
        answer: oneshot::Sender<Response>,
    ) -> Option<oneshot::Sender<Response>> {
        let Some(limit) = newest_of_ops(read) else {
            return Some(answer);
        };
        // Without its echo, this machine's own messages wouldn't be heard: nothing is kept.
        if !self
            .client
            .as_ref()
            .is_some_and(|client| client.has_cap("echo-message"))
        {
            self.drop_ops();
            return Some(answer);
        }
        match &mut self.ops {
            Some(kept) if limit <= kept.limit && kept.read => {
                let messages = kept.newest(limit).unwrap_or_default();
                let _ = answer.send(Response::Messages { messages });
                None
            }
            Some(kept) if limit <= kept.limit => {
                kept.waiting.push((limit, answer));
                None
            }
            // More than is being read: the server's to answer.
            Some(kept) if !kept.read => Some(answer),
            _ => {
                self.drop_ops();
                let since = unix().saturating_sub(STATES_BACK);
                let mut kept = Kept::reading(limit.max(states::LOOK_BACK), since);
                let fill = kept.filling();
                kept.waiting.push((limit, answer));
                self.ops = Some(kept);
                // Nobody waits on its answer: it's kept, and answers those waiting.
                let (filled, _) = oneshot::channel();
                self.read(fill, filled);
                None
            }
        }
    }

    /// A read of `target` through `range` was answered with `response`: kept, when it's the
    /// read keeping `#ops` waits for. If it failed, nothing is kept.
    pub(super) fn read_in(&mut self, target: &str, range: history::Range, response: &Response) {
        let Some(kept) = &mut self.ops else { return };
        let keeping = target.eq_ignore_ascii_case(states::CHANNEL)
            && range.since == Some(kept.since)
            && range.until.is_none()
            && !kept.read
            && range.limit == Some(READ_BACK);
        if !keeping {
            return;
        }
        match response {
            Response::Messages { messages } => kept.read(messages.clone()),
            _ => self.drop_ops(),
        }
    }

    /// Reads `#ops`'s newest, to keep them, as this machine joins it.
    pub(super) fn keep_ops(&mut self, channel: &str) {
        if !channel.eq_ignore_ascii_case(states::CHANNEL) || self.ops.is_some() {
            return;
        }
        let read = newest(states::LOOK_BACK);
        // Nobody waits on it: its answer is only kept.
        let (answer, _) = oneshot::channel();
        if let Some(answer) = self.read_kept(&read, answer) {
            self.read(read, answer);
        }
    }

    /// Forgets what's kept of `#ops`: reads waiting for it ask the server.
    pub(super) fn drop_ops(&mut self) {
        let Some(kept) = self.ops.take() else { return };
        // Kept till it's read again, so an organisation that said no isn't taken as yes
        // meanwhile.
        self.colleagues_said = kept.said_colleagues().or(self.colleagues_said);
        for (limit, answer) in kept.waiting {
            self.read(newest(limit), answer);
        }
    }

    /// A line from the server, as it bears on what's kept of `#ops`: a message heard there,
    /// as `delivery`, is kept, and a trim there drops it all.
    pub(super) fn ops_heard(&mut self, line: &Line, delivery: Option<&Delivery>) {
        if !line
            .params
            .first()
            .is_some_and(|channel| channel.eq_ignore_ascii_case(states::CHANNEL))
        {
            return;
        }
        match line.command.as_str() {
            "REDACT" => self.drop_ops(),
            "PRIVMSG" if line.tag("batch").is_none() => {
                if let (Some(kept), Some(delivery)) = (&mut self.ops, delivery) {
                    kept.heard(delivery.clone());
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use zirc_proto::{Event, Message};

    use super::*;

    fn said(id: &str) -> Delivery {
        Delivery {
            id: Some(id.into()),
            ..Delivery::new(
                "#ops",
                "ci",
                Message::new("", Event::Text { body: id.into() }),
            )
        }
    }

    fn ids(messages: &[Delivery]) -> Vec<&str> {
        messages.iter().filter_map(|it| it.id.as_deref()).collect()
    }

    #[test]
    fn answers_nothing_until_the_read_is_in() {
        let mut kept = Kept::reading(3, 0);
        kept.heard(said("c"));
        assert_eq!(kept.newest(3), None);
        // "c" came while the server was answering: the read may have it, or not.
        kept.read(vec![said("a"), said("b")]);
        assert_eq!(ids(&kept.newest(3).unwrap()), ["a", "b", "c"]);
        let mut again = Kept::reading(3, 0);
        again.heard(said("c"));
        again.read(vec![said("b"), said("c")]);
        assert_eq!(ids(&again.newest(3).unwrap()), ["b", "c"]);
    }

    #[test]
    fn keeps_the_newest_as_they_come_and_no_more() {
        let mut kept = Kept::reading(3, 0);
        kept.read(vec![said("a"), said("b"), said("c")]);
        kept.heard(said("d"));
        kept.heard(said("d"));
        assert_eq!(ids(&kept.newest(3).unwrap()), ["b", "c", "d"]);
        assert_eq!(ids(&kept.newest(1).unwrap()), ["d"]);
        // More than it keeps: the server is asked.
        assert_eq!(kept.newest(4), None);
    }

    fn state_said(id: &str, name: &str, time: &str) -> Delivery {
        let state = zirc_proto::state::State {
            repo: "#monorepo".into(),
            name: name.into(),
            level: zirc_proto::state::Level::Broken,
            why: None,
            owner: None,
            link: None,
            until: None,
        };
        Delivery {
            id: Some(id.into()),
            time: Some(time.into()),
            ..Delivery::new("#ops", "ci", Message::new("ci", state.event()))
        }
    }

    #[test]
    fn a_state_said_before_whats_kept_is_still_read_with_states() {
        let mut kept = Kept::reading(states::LOOK_BACK, 0);
        kept.read(vec![
            state_said("held", "landings", "2026-11-01T00:00:00Z"),
            state_said("old", "master", "2026-11-01T00:00:01Z"),
            state_said("new", "master", "2026-11-01T00:00:02Z"),
        ]);
        for n in 0..states::LOOK_BACK {
            kept.heard(said(&format!("m{n}")));
        }
        let read = kept.newest(states::LOOK_BACK).unwrap();
        assert_eq!(read.len(), states::LOOK_BACK + 2);
        assert_eq!(ids(&read[..3]), ["held", "new", "m0"]);
        // A read of fewer, which isn't a read of states, has the newest alone.
        assert_eq!(ids(&kept.newest(2).unwrap()), ["m998", "m999"]);
    }

    /// What answering `zirc state` from what's kept costs: the newest 1000 and their JSON,
    /// as the socket sends them. Printed with `--nocapture`; the limit catches a quadratic
    /// return, not noise: a loaded CI agent took 54 to 164 ms where a quiet machine takes
    /// about 15 (builds 3172 and 3191), and a quadratic return takes seconds.
    #[test]
    fn answering_from_whats_kept_takes_well_under_a_millisecond_a_message() {
        let mut kept = Kept::reading(states::LOOK_BACK, 0);
        kept.read(Vec::new());
        for n in 0..states::LOOK_BACK * 2 {
            let mut delivery = said(&format!("m{n}"));
            delivery.message = Message::new(
                "ci",
                Event::Text {
                    body: format!("#monorepo master is broken: //web:e2e fails since build {n}"),
                },
            );
            kept.heard(delivery);
        }
        let started = std::time::Instant::now();
        let runs = 20;
        let mut size = 0;
        for _ in 0..runs {
            let messages = kept.newest(states::LOOK_BACK).unwrap();
            size = serde_json::to_string(&Response::Messages { messages })
                .unwrap()
                .len();
        }
        let each = started.elapsed().as_secs_f64() * 1000.0 / f64::from(runs);
        println!(
            "PERF #ops's newest 1000 from what's kept, as JSON: {each:.3} ms, {} KB",
            size / 1024
        );
        assert!(each < 1000.0, "{each} ms");
    }
}
