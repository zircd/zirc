//! Purging channels a team no longer uses, with everything said in them: the rooms
//! repositories had before 20 Nov 2026 (`#<repo>`, `#<repo>/agents`…), which the user chose
//! to delete once every agent spoke in the organisation's `#agents`. Only asked for, by name
//! (`POST /v1/channels/purge`, which `//ops:purge-channels` sends), never by the server itself.
//!
//! A channel goes in two steps, as the server's operator: every message Ergo's history keeps
//! under it is deleted (`HistServ DELETE`), then `ChanServ PURGE` takes the channel itself,
//! putting out whoever's in it and keeping it from being made again. The organisation's own
//! rooms are never purged ([`KEPT`]).

use crate::ergo::{Operator, OperatorError};
use crate::janitor::{confirmation_of, plain};

/// The organisation's own rooms, which every server has and nothing purges.
pub const KEPT: &[&str] = &[
    zirc_proto::ORG_CHANNEL,
    zirc_proto::AGENTS_CHANNEL,
    zirc_proto::gotchas::CHANNEL,
    zirc_proto::state::CHANNEL,
    zirc_proto::request::CHANNEL,
    zirc_proto::workflow::CHANNEL,
];

/// Why `channel` can't be purged, if it can't: it isn't a channel's name, or it's one of the
/// organisation's own rooms ([`KEPT`]).
#[must_use]
pub fn refused(channel: &str) -> Option<String> {
    if !channel.starts_with('#') || channel.len() < 2 || channel.contains([' ', ',', '\x07']) {
        return Some(format!("{channel:?} isn't a channel's name"));
    }
    KEPT.iter()
        .any(|kept| kept.eq_ignore_ascii_case(channel))
        .then(|| format!("{channel} is the organisation's own room: it's never purged"))
}

/// What purging one channel did.
#[derive(Debug, serde::Serialize)]
pub struct Purged {
    /// The channel.
    pub channel: String,
    /// How many of its messages Ergo's history no longer has.
    pub deleted: usize,
    /// What `ChanServ` said of purging it.
    pub chanserv: String,
}

/// Purges `channel`: deletes the messages `msgids` (all of its history) as `operator`, then has
/// `ChanServ` purge the channel, confirming with the code it asks for.
///
/// # Errors
///
/// When the operator can't log in or a service doesn't answer, or `ChanServ` refuses.
pub async fn purge(
    operator: &Operator,
    channel: &str,
    msgids: Vec<String>,
) -> Result<Purged, OperatorError> {
    let messages: Vec<(String, String)> = msgids
        .into_iter()
        .map(|msgid| (channel.to_owned(), msgid))
        .collect();
    let deleted = operator.delete_messages(&messages).await?;
    // No reason after the code: Ergo splits PURGE into at most three words and the rest, so a
    // reason would be read as part of the code (irc/chanserv.go, "purge"'s maxParams).
    let asked = format!("PURGE ADD {channel}");
    let mut answer = plain(
        &operator
            .chanserv(std::slice::from_ref(&asked))
            .await?
            .concat(),
    );
    // ChanServ asks to confirm with a code it gives, in its second notice:
    // `To confirm, run this command: /CS PURGE ADD <channel> <code>`.
    if let Some(code) = confirmation_of(&answer, "purge add", channel) {
        answer = plain(
            &operator
                .chanserv(&[format!("PURGE ADD {channel} {code}")])
                .await?
                .concat(),
        );
    }
    if answer.to_lowercase().contains("purged") {
        Ok(Purged {
            channel: channel.to_owned(),
            deleted,
            chanserv: answer,
        })
    } else {
        Err(OperatorError::Failed {
            code: "PURGE".into(),
            message: answer,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_purges_the_organisations_own_rooms_or_what_isnt_a_channel() {
        for kept in [
            "#general",
            "#agents",
            "#Gotchas",
            "#ops",
            "#decisions",
            "#workflows",
        ] {
            assert!(refused(kept).is_some(), "{kept}");
        }
        for bad in ["monorepo", "#", "#a b", "#a,#b"] {
            assert!(refused(bad).is_some(), "{bad}");
        }
        for old in [
            "#monorepo",
            "#monorepo/agents",
            "#monorepo/landings",
            "#zirc",
        ] {
            assert_eq!(refused(old), None, "{old}");
        }
    }

    #[test]
    fn reads_the_code_chanserv_asks_to_confirm_a_purge_with() {
        // Ergo 2.19.1's two notices (irc/chanserv.go, csPurgeAddHandler), as ChanServ's
        // answer reads them.
        let asked = "Warning: you are about to empty this channel and remove it from the server.\n\
                     To confirm, run this command: /CS PURGE ADD #monorepo/agents xY7kQ2";
        assert_eq!(
            confirmation_of(asked, "purge add", "#monorepo/agents").as_deref(),
            Some("xY7kQ2")
        );
        assert_eq!(confirmation_of(asked, "purge add", "#monorepo"), None);
    }
}
