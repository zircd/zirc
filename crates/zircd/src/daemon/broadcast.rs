//! A broadcast, which reaches every session hearing it, as in the agents' room to reach every
//! agent at once: a person's wakes them all; an agent's comes with each session's next wake
//! (`later.rs`), and wakes only whom it names. An agent broadcasts with `zirc post
//! --everyone`, a flag on its message
//! ([`zirc_proto::Message::everyone`]): `@everyone` in an agent's text is only text, so a
//! quote, a code block or a post about it never broadcasts (it did, a few times). A person's
//! own words saying `@everyone` do, as they mean it. Text an event carries, a decision's quote
//! or a card's, never does. And one sender broadcasts at most once every [`EVERY`]: a
//! broadcast sooner reaches every watch, but wakes only whom it names. The sender is who the
//! server says sent it, its account or else its nick, never a field the sender's own frame
//! sets, such as its session, which a machine could change with each post.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use zirc_core::chat::{SenderKind, sender_kind};
use zirc_core::ipc::{Delivery, Response};
use zirc_proto::Event;

use super::{Daemon, EVERYONE, mentions};

/// How often one sender's `@everyone` wakes every session, at most.
pub(super) const EVERY: Duration = Duration::from_mins(10);

/// When each sender, by account or nick, last broadcast.
pub(super) type Broadcasts = BTreeMap<String, Instant>;

/// Who sent `delivery`, as broadcasts are counted: the account the server says, or else the
/// nick, which the server holds to one connection at a time.
fn sender(delivery: &Delivery) -> String {
    delivery
        .account
        .clone()
        .unwrap_or_else(|| delivery.from.clone())
}

/// Whether `delivery` is for every session: an agent's post flagged so, or a person's own
/// words saying `@everyone`. A person's line carries no session, but a machine's frame with
/// none is still a machine's.
fn says_everyone(delivery: &Delivery) -> bool {
    let Event::Text { body } = &delivery.message.event else {
        return false;
    };
    let persons = delivery.message.session.is_empty()
        && sender_kind(&delivery.from, delivery.hosted_by.as_deref()) == SenderKind::Person;
    delivery.message.everyone || (persons && mentions(body, &[EVERYONE]))
}

impl Daemon {
    /// Whether `delivery` is a broadcast that wakes every session ([`says_everyone`]), from a
    /// sender who hasn't broadcast within [`EVERY`].
    pub(super) fn broadcast(&self, delivery: &Delivery) -> bool {
        says_everyone(delivery)
            && self
                .broadcasts
                .get(&sender(delivery))
                .is_none_or(|at| self.clock.now().duration_since(*at) >= EVERY)
    }

    /// Notes `response`, once told, as its sender's latest broadcast, if it was one.
    pub(super) fn broadcast_told(&mut self, response: &Response) {
        if let Response::Delivery(delivery) = response
            && self.broadcast(delivery)
        {
            // Only the senders still within the limit need remembering.
            let now = self.clock.now();
            self.broadcasts
                .retain(|_, at| now.duration_since(*at) < EVERY);
            self.broadcasts.insert(sender(delivery), now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zirc_proto::Message;

    fn said(from: &str, account: Option<&str>, session: &str, text: &str) -> Delivery {
        // A machine, as its host says: here, any nick ending `-mbp`.
        let hosted_by = from.strip_suffix("-mbp").map(str::to_owned);
        Delivery {
            account: account.map(Into::into),
            hosted_by,
            ..Delivery::new(
                "#api/agents",
                from,
                Message::new(session, Event::Text { body: text.into() }),
            )
        }
    }

    #[test]
    fn only_a_persons_own_words_broadcast_without_the_flag() {
        assert!(says_everyone(&said("ada", None, "", "@everyone: lunch")));
        // A machine's frame naming no session is still a machine's.
        assert!(!says_everyone(&said(
            "ada-mbp",
            None,
            "",
            "@everyone: lunch"
        )));
        let mut flagged = said("ada-mbp", None, "cc-1", "main is frozen");
        flagged.message.everyone = true;
        assert!(says_everyone(&flagged));
    }

    /// Timed by the daemon's clock, which the test moves rather than wait ten minutes.
    #[test]
    fn a_sender_broadcasts_again_only_once_the_limit_has_passed() {
        let home = tempfile::tempdir().unwrap();
        let org = super::super::Org {
            name: "zirc".into(),
            first: true,
            state: home.path().to_owned(),
            several: false,
        };
        let (inputs, _received) = tokio::sync::mpsc::unbounded_channel();
        let mut daemon = Daemon::new(
            home.path().to_owned(),
            org,
            "irc.example".into(),
            None,
            inputs,
        );
        let lunch = said("ada", Some("ada"), "", "@everyone: lunch");
        assert!(daemon.broadcast(&lunch));
        daemon.broadcast_told(&Response::Delivery(lunch.clone()));
        assert!(!daemon.broadcast(&lunch), "again at once");
        daemon.clock.advance(EVERY / 2);
        assert!(!daemon.broadcast(&lunch), "halfway to the limit");
        daemon.clock.advance(EVERY / 2);
        assert!(daemon.broadcast(&lunch));
    }

    #[test]
    fn counts_broadcasts_by_account_not_by_session() {
        let one = said("ada-mbp", Some("ada"), "cc-1", "");
        let two = said("ada-mbp2", Some("ada"), "cc-2", "");
        assert_eq!(sender(&one), sender(&two));
        assert_eq!(sender(&said("bob-mbp", None, "cc-1", "")), "bob-mbp");
    }
}
