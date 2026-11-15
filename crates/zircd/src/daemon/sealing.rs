//! Sealed parts of posts ([`zirc_core::sealed`]): sealed with this machine's key for its
//! organisation as they're sent, and opened for `zirc unseal` from the messages heard lately.

use anyhow::Result;
use zirc_core::ipc::Response;
use zirc_core::sealed::{Context, Keys, Opened, machine_store, no_key, open_message};
use zirc_proto::sealed::CLIENT_TAGS_MAX;

use super::Daemon;

/// The tag a reply goes with, whose room a sealed reply's parts leave.
const REPLY_TAG: &str = "+draft/reply";

impl Daemon {
    /// The `+zirc.dev/sealed` tag's value for `parts` of a post to `channel`, replying to
    /// `reply` if it does; `None` when there are none.
    pub(super) fn seal_for(
        &self,
        channel: &str,
        parts: &[String],
        reply: Option<&str>,
    ) -> Result<Option<String>> {
        if parts.is_empty() {
            return Ok(None);
        }
        let org = &self.org.name;
        let store = machine_store(&self.home);
        let key = Keys::new(&store)
            .sealing(org)
            .ok_or_else(|| anyhow::anyhow!(no_key(org)))?;
        let context = Context { target: channel };
        // A reply's tag, and the `;` between it and this one.
        let taken = reply.map_or(0, |id| REPLY_TAG.len() + 1 + id.len() + 1);
        let room = CLIENT_TAGS_MAX.saturating_sub(taken);
        Ok(Some(zirc_core::sealed::seal_message(
            &key, &context, parts, room,
        )?))
    }

    /// Opens part `part`, from 1, of the message `message` names ([`zirc_core::ipc::Request::Unseal`]).
    pub(super) fn unseal(&self, message: &str, part: usize) -> Result<Response> {
        let said = self.recent.find(message)?;
        let Some(sealed) = &said.sealed else {
            anyhow::bail!("{} has no sealed parts", said.id);
        };
        let org = &self.org.name;
        let store = machine_store(&self.home);
        let keys = Keys::new(&store).of(org);
        let context = Context {
            target: &said.target,
        };
        let opened = open_message(&keys, &context, sealed).unwrap_or_default();
        let count = opened.len();
        let Some(opened) = part
            .checked_sub(1)
            .and_then(|at| opened.into_iter().nth(at))
        else {
            anyhow::bail!(
                "{} has {count} sealed part{}: give one from 1 to {count}",
                said.id,
                if count == 1 { "" } else { "s" }
            );
        };
        match opened {
            Opened::Value(value) => Ok(Response::Unsealed { value }),
            Opened::NoKey if keys.is_empty() => anyhow::bail!(no_key(org)),
            Opened::NoKey => anyhow::bail!(
                "this machine doesn't hold the key {} was sealed with: add it with `zirc team-key add`",
                said.id
            ),
            Opened::Failed => anyhow::bail!(
                "{}'s part {part} couldn't be opened: it was changed, or moved to another channel",
                said.id
            ),
        }
    }
}
