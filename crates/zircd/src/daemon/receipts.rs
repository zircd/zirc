//! Saying how a message to one of this machine's sessions stands: told to its watch, or
//! waiting for a session with no watch open ([`zirc_proto::receipt`]). Nothing said either
//! until 22 Nov 2026, so a message to a stopped agent looked delivered to whoever sent it.
//!
//! Only a message for one session gets a receipt: a person's direct message naming it, a
//! note, or a query asked of it. An answer gets none, nor a line for every session.

use zirc_core::ipc::{Delivery, Direct, Response};
use zirc_irc::Line;
use zirc_proto::receipt::{Receipt, State, TAG};
use zirc_proto::{Event, Message};

use super::{Daemon, WATCHER_SESSION};

/// Whether `direct` is one a receipt is said for, and the session it's for.
fn receipted(direct: &Direct) -> Option<&str> {
    direct
        .answer
        .is_none()
        .then_some(direct.session.as_deref())
        .flatten()
}

impl Daemon {
    /// Tells the watchers `direct`, then says how it stands to whoever sent it.
    pub(super) fn tell_direct(&mut self, direct: Direct) {
        let response = Response::Direct(direct);
        self.tell_watchers(&response);
        let Response::Direct(direct) = response else {
            return;
        };
        let Some(session) = receipted(&direct) else {
            return;
        };
        let watching = self
            .watchers
            .iter()
            .any(|watcher| watcher.reachable() && watcher.session.as_deref() == Some(session));
        // Kept only for a session that has watched: one that never has is told nothing
        // later either, so nothing is said of it.
        let kept = self.missed.get(session).is_some_and(|kept| {
            kept.iter().any(|(_, kept)| {
                matches!(kept, Response::Direct(kept)
                    if kept.id == direct.id && kept.text == direct.text)
            })
        });
        let state = match (watching, kept) {
            (true, _) => State::Seen,
            (false, true) => State::Waiting,
            (false, false) => return,
        };
        self.say_receipt(&direct, state);
    }

    /// Says each of `missed` that was for `session` alone is seen, now that its watch has
    /// been told what it missed.
    pub(super) fn say_seen(&mut self, missed: &[Response]) {
        for response in missed {
            if let Response::Direct(direct) = response
                && receipted(direct).is_some()
            {
                self.say_receipt(direct, State::Seen);
            }
        }
    }

    /// Says `direct` stands as `state` to whoever sent it: by a `TAGMSG` naming the
    /// message, or, sent from a session here, by telling that session if it waits.
    fn say_receipt(&mut self, direct: &Direct, state: State) {
        let Some(session) = receipted(direct) else {
            return;
        };
        let receipt = Receipt {
            state,
            session: session.to_owned(),
            sender: direct.from_session.clone(),
        };
        let Some(me) = self.client.as_ref().map(|client| client.nick().to_owned()) else {
            return;
        };
        if direct.from.eq_ignore_ascii_case(&me) {
            self.receipt_told(&me, &receipt);
            return;
        }
        let (Some(id), Some(client)) = (&direct.id, &self.client) else {
            return;
        };
        let mut line = Line::new("TAGMSG", [direct.from.as_str()]);
        line.tags = vec![
            (TAG.to_owned(), Some(receipt.value())),
            ("+draft/reply".to_owned(), Some(id.clone())),
        ];
        if let Err(err) = client.send(line) {
            tracing::debug!("couldn't say {} to {}: {err}", receipt.value(), direct.from);
        }
    }

    /// Takes a `TAGMSG` from `from` as a receipt, if it is one: a message one of this
    /// machine's sessions sent waits there. Only from an account the server names.
    pub(super) fn heard_receipt(&mut self, from: &str, line: &Line) -> bool {
        let Some(value) = line.tag(TAG) else {
            return false;
        };
        if line.tag("account").is_some()
            && let Some(receipt) = Receipt::parse(value)
        {
            self.receipt_told(from, &receipt);
        }
        true
    }

    /// Tells the session here that sent a message now waiting on `machine` that it does,
    /// as a line of this machine's own, addressed to it. A message seen wakes nobody.
    fn receipt_told(&mut self, machine: &str, receipt: &Receipt) {
        let Some(body) = receipt.waits(machine) else {
            return;
        };
        if receipt
            .sender
            .as_deref()
            .and_then(|sender| self.local_session(sender))
            .is_none()
        {
            return;
        }
        let Some(me) = self.client.as_ref().map(|client| client.nick().to_owned()) else {
            return;
        };
        self.tell_watchers(&Response::Delivery(Delivery::new(
            zirc_core::project::ORG_CHANNEL,
            me,
            Message::new(WATCHER_SESSION, Event::Text { body }),
        )));
    }
}
