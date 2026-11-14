//! How sure a decision ([`zirc_proto::decision`]) is the decider's own word, judged by the
//! account the server says sent it, never by what the decision claims. Only a person's own
//! account speaks for them; a terminal on their machine is as sure as that machine; an
//! agent session relaying them is that session's account of what they said; anyone else
//! saying so is no decision of theirs at all.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zirc_proto::decision::Decision;

use crate::agents::{Sender, Standing, standing};

/// Where decisions go and are looked up: the organisation's `#decisions`, a noticeboard
/// holding nothing else, so a decision isn't lost among alerts and deploys.
pub const CHANNEL: &str = "#decisions";

/// Where decisions went until 6 Nov 2026, and machines on older zirc still send them:
/// `#ops`, which `zirc decisions` reads too.
pub const EARLIER: &str = "#ops";

/// Who sent a decision, as far as the server says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Provenance {
    /// Sent from the decider's own account: their own words, as the server says.
    Own,
    /// Sent from a terminal on one of the decider's machines, as its zircd says: as sure as
    /// that machine is.
    Terminal {
        /// The machine's account.
        machine: String,
    },
    /// Relayed by an agent session on one of the decider's machines: what that session says
    /// they told it. Ask them before acting on it for anything costly or hard to undo.
    Relayed {
        /// The machine's account.
        machine: String,
        /// The session.
        session: String,
    },
    /// From no account, or one that isn't the decider's or one of their machines': someone
    /// else's say-so, which decides nothing.
    Unverified,
}

impl Provenance {
    /// In a few words: `their own account`, `a terminal on mattias-mbp`, `relayed by
    /// mattias-mbp/cc-1`, `unverified`.
    #[must_use]
    pub fn words(&self) -> String {
        match self {
            Self::Own => "their own account".to_owned(),
            Self::Terminal { machine } => format!("a terminal on {machine}"),
            Self::Relayed { machine, session } => format!("relayed by {machine}/{session}"),
            Self::Unverified => "unverified".to_owned(),
        }
    }
}

/// How sure `decision` is its decider's word, sent by `sender` as the server says, on a
/// server whose accounts only zirc.dev makes when `named_by_zirc_dev` (see
/// [`crate::agents::standing`]): only then is a machine's account, or a client's, surely
/// theirs.
#[must_use]
pub fn provenance(decision: &Decision, sender: Sender<'_>, named_by_zirc_dev: bool) -> Provenance {
    let machine = match standing(sender, Some(&decision.by), named_by_zirc_dev) {
        Standing::Owner => return Provenance::Own,
        Standing::Machine => sender.account().unwrap_or_default().to_owned(),
        Standing::Member | Standing::Nobody => return Provenance::Unverified,
    };
    match &decision.session {
        Some(session) => Provenance::Relayed {
            machine,
            session: session.clone(),
        },
        None => Provenance::Terminal { machine },
    }
}

#[cfg(test)]
mod tests {
    use zirc_proto::names::Hosted;

    use super::*;

    fn decision(session: Option<&str>) -> Decision {
        Decision {
            by: "mattias".into(),
            quote: "yes, add the node".into(),
            about: "cloud spend".into(),
            session: session.map(Into::into),
            link: None,
            for_: Vec::new(),
            replaces: None,
        }
    }

    #[test]
    fn trusts_the_account_not_what_the_decision_says() {
        let relayed = decision(Some("cc-1"));
        assert_eq!(
            provenance(&relayed, Sender::new(Some("mattias"), None), true),
            Provenance::Own
        );
        // Their machine, of any name, as its host the server set says.
        assert_eq!(
            provenance(
                &relayed,
                Sender::new(Some("m3-mac"), Some(Hosted::Machine("mattias"))),
                true
            ),
            Provenance::Relayed {
                machine: "m3-mac".into(),
                session: "cc-1".into()
            }
        );
        assert_eq!(
            provenance(
                &decision(None),
                Sender::new(Some("m3-mac"), Some(Hosted::Machine("mattias"))),
                true
            ),
            Provenance::Terminal {
                machine: "m3-mac".into()
            }
        );
        // His own client under another name, as its host says: his word as his account's is.
        let client = Sender::new(Some("mattias-irc"), Some(Hosted::Client("mattias")));
        assert_eq!(provenance(&relayed, client, true), Provenance::Own);
        assert_eq!(provenance(&relayed, client, false), Provenance::Unverified);
        // Someone else, their machine, or a name alone, saying mattias decided: not his word.
        assert_eq!(
            provenance(&relayed, Sender::new(Some("bob"), None), true),
            Provenance::Unverified
        );
        assert_eq!(
            provenance(
                &relayed,
                Sender::new(Some("m3-mac"), Some(Hosted::Machine("bob"))),
                true
            ),
            Provenance::Unverified
        );
        assert_eq!(
            provenance(&relayed, Sender::new(Some("mattias-mbp"), None), true),
            Provenance::Unverified
        );
        assert_eq!(
            provenance(&relayed, Sender::new(None, None), true),
            Provenance::Unverified
        );
        // Where accounts aren't zirc.dev's, a machine's host proves nothing.
        assert_eq!(
            provenance(
                &relayed,
                Sender::new(Some("m3-mac"), Some(Hosted::Machine("mattias"))),
                false
            ),
            Provenance::Unverified
        );
        assert_eq!(
            provenance(&relayed, Sender::new(Some("Mattias"), None), false),
            Provenance::Own
        );
        assert_eq!(
            Provenance::Relayed {
                machine: "m".into(),
                session: "s".into()
            }
            .words(),
            "relayed by m/s"
        );
    }
}
