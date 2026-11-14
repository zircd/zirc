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

use std::collections::VecDeque;

use tokio::sync::oneshot;
use zirc_core::ipc::{Delivery, Request, Response};
use zirc_core::{history, states};
use zirc_irc::Line;

use super::Daemon;

/// The newest messages of `#ops`, or those heard while they're being read.
#[derive(Debug, Default)]
pub(super) struct Kept {
    /// The most kept: the read's limit.
    limit: usize,
    /// Oldest first, as history comes.
    messages: VecDeque<Delivery>,
    /// Whether the read is in: until then, `messages` holds only those heard meanwhile.
    read: bool,
    /// Reads waiting for it, each with its limit.
    waiting: Vec<(usize, oneshot::Sender<Response>)>,
}

impl Kept {
    /// About to read the newest `limit`: what's heard meanwhile is kept, to add to them.
    pub(super) fn reading(limit: usize) -> Self {
        Self {
            limit,
            ..Self::default()
        }
    }

    /// The read is in: `history`, then each heard meanwhile that it doesn't already have.
    /// Those waiting for it are answered.
    pub(super) fn read(&mut self, history: Vec<Delivery>) {
        let heard = std::mem::replace(&mut self.messages, history.into());
        self.read = true;
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
        self.messages.push_back(delivery);
        if self.read {
            while self.messages.len() > self.limit {
                self.messages.pop_front();
            }
        }
    }

    /// The newest `limit` messages, if they're all here.
    pub(super) fn newest(&self, limit: usize) -> Option<Vec<Delivery>> {
        (self.read && limit <= self.limit).then(|| {
            let skip = self.messages.len().saturating_sub(limit);
            self.messages.iter().skip(skip).cloned().collect()
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

/// A read of `#ops`'s newest `limit`.
fn newest(limit: usize) -> Request {
    Request::Read {
        target: states::CHANNEL.to_owned(),
        limit,
        since: None,
        until: None,
    }
}

impl Daemon {
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
                self.ops = Some(Kept::reading(limit));
                Some(answer)
            }
        }
    }

    /// A read of `target` through `range` was answered with `response`: kept, when it's the
    /// read keeping `#ops` waits for. If it failed, nothing is kept.
    pub(super) fn read_in(&mut self, target: &str, range: history::Range, response: &Response) {
        let Some(kept) = &mut self.ops else { return };
        let keeping = target.eq_ignore_ascii_case(states::CHANNEL)
            && range.since.is_none()
            && range.until.is_none()
            && !kept.read
            && range.limit == Some(kept.limit);
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
        let mut kept = Kept::reading(3);
        kept.heard(said("c"));
        assert_eq!(kept.newest(3), None);
        // "c" came while the server was answering: the read may have it, or not.
        kept.read(vec![said("a"), said("b")]);
        assert_eq!(ids(&kept.newest(3).unwrap()), ["a", "b", "c"]);
        let mut again = Kept::reading(3);
        again.heard(said("c"));
        again.read(vec![said("b"), said("c")]);
        assert_eq!(ids(&again.newest(3).unwrap()), ["b", "c"]);
    }

    #[test]
    fn keeps_the_newest_as_they_come_and_no_more() {
        let mut kept = Kept::reading(3);
        kept.read(vec![said("a"), said("b"), said("c")]);
        kept.heard(said("d"));
        kept.heard(said("d"));
        assert_eq!(ids(&kept.newest(3).unwrap()), ["b", "c", "d"]);
        assert_eq!(ids(&kept.newest(1).unwrap()), ["d"]);
        // More than it keeps: the server is asked.
        assert_eq!(kept.newest(4), None);
    }

    /// What answering `zirc state` from what's kept costs: the newest 1000 and their JSON,
    /// as the socket sends them. Printed with `--nocapture`; the limit catches a quadratic
    /// return, not noise: a loaded CI agent took 54 to 164 ms where a quiet machine takes
    /// about 15 (builds 3172 and 3191), and a quadratic return takes seconds.
    #[test]
    fn answering_from_whats_kept_takes_well_under_a_millisecond_a_message() {
        let mut kept = Kept::reading(states::LOOK_BACK);
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
