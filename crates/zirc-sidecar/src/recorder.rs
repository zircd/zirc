//! The tenant's recorder: one connection to Ergo on the pod's loopback, as
//! [`zirc_proto::activity::RECORDER`], that hears the activity records machines send it
//! ([`zirc_proto::activity`]: wakes, what sessions did after, prompts, notices of overlaps) and keeps a row of each
//! in the activity store ([`crate::activity`]). They're debug, so live only: sent as a
//! `TAGMSG` to the recorder alone, never kept in Ergo's history or seen by anyone else.
//!
//! Its account is made when it's first needed, with a passphrase derived from the endpoint's
//! token, as an integration's is ([`crate::say`]), so none is stored, and its nick is
//! reserved ([`zirc_proto::names::RESERVED_INTEGRATION_NICKS`]), so no team's hook can post
//! as it. It takes only records of zirc's activity kinds, each from a logged-in account, and
//! joins no channel. While it's away, what's sent to it is lost: Ergo answers the sender
//! that there's no such nick.

use std::path::PathBuf;
use std::time::Duration;

use tracing::{info, warn};
use zirc_irc::Line;
use zirc_proto::activity::RECORDER;

/// How long it waits before connecting again, at first and at most.
const RETRY: Duration = Duration::from_secs(5);
const RETRY_MOST: Duration = Duration::from_mins(5);

/// What the recorder needs to connect: Ergo's loopback address, its gate password, and the
/// endpoint's token its passphrase is derived from.
#[derive(Clone)]
pub struct Recorder {
    /// Ergo's loopback address.
    pub host: String,
    /// Its IRC port there.
    pub port: u16,
    /// The server's gate password, if it has one.
    pub gate: Option<String>,
    /// The endpoint's token, from which its passphrase is derived.
    pub token: String,
    /// Ergo's history, beside which the activity store is kept.
    pub history: PathBuf,
}

/// The activity record `line` carries for the recorder, with its sender's account and the
/// server's time of it in nanoseconds, or `None` for any other line.
#[must_use]
pub fn record_in(line: &Line) -> Option<(Option<String>, i64, zirc_proto::Message)> {
    if line.command != "TAGMSG"
        || !line
            .params
            .first()
            .is_some_and(|target| target.eq_ignore_ascii_case(RECORDER))
    {
        return None;
    }
    let message = zirc_proto::from_payload(line.tag(zirc_proto::EVENT_TAG)?).ok()?;
    let account = line
        .tag("account")
        .map(str::to_lowercase)
        .filter(|account| !account.is_empty() && account != "*");
    let nanotime = line
        .tag("time")
        .and_then(|time| time.parse::<jiff::Timestamp>().ok())
        .unwrap_or_else(jiff::Timestamp::now)
        .as_nanosecond();
    Some((account, i64::try_from(nanotime).ok()?, message))
}

impl Recorder {
    /// Records what's sent to it until the process ends, connecting again whenever the
    /// connection drops. `account` makes sure its account exists with the derived
    /// passphrase (`reset` sets it again, as when the token changed).
    pub async fn run<F, Fut>(self, account: F)
    where
        F: Fn(bool) -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let mut wait = RETRY;
        let mut reset = false;
        loop {
            if account(reset).await {
                match self.once().await {
                    Ok(()) => wait = RETRY,
                    Err(zirc_irc::Error::Sasl(why)) => {
                        warn!(%why, "the recorder's login was refused: setting its passphrase again");
                        reset = true;
                    }
                    Err(error) => warn!(%error, "the recorder's connection ended"),
                }
            }
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(RETRY_MOST);
        }
    }

    async fn once(&self) -> Result<(), zirc_irc::Error> {
        let config = crate::say::config(
            &self.host,
            self.port,
            self.gate.as_deref(),
            &self.token,
            RECORDER,
        );
        let (_client, mut incoming) = zirc_irc::connect(&config).await?;
        info!("the recorder is listening");
        let store = crate::activity::path_for(&self.history);
        while let Some(line) = incoming.recv().await {
            let Some((account, nanotime, message)) = record_in(&line) else {
                continue;
            };
            let store = store.clone();
            let kept = tokio::task::spawn_blocking(move || {
                let connection = crate::activity::open(&store)?;
                crate::activity::record_live(&connection, nanotime, account, &message)
            })
            .await;
            match kept {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => warn!(%error, "the recorder couldn't keep a record"),
                Err(error) => warn!(%error, "keeping a record didn't finish"),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zirc_proto::activity::{Acted, Record, WakeOutcome};

    fn tagged(target: &str, message: &zirc_proto::Message) -> Line {
        let mut line = Line::new("TAGMSG", [target]);
        line.source = Some("ada-mbp!u@h".into());
        line.tags = vec![
            ("account".into(), Some("Ada-MBP".into())),
            ("time".into(), Some("2026-11-08T12:00:00.000Z".into())),
            (
                zirc_proto::EVENT_TAG.into(),
                Some(zirc_proto::payload(message).unwrap()),
            ),
        ];
        line
    }

    #[test]
    fn takes_what_is_sent_to_it_alone() {
        let outcome = zirc_proto::Message::new(
            "cc-1",
            WakeOutcome {
                wake: "w1".into(),
                acted: Acted::Posted,
                after: Some(40),
            }
            .event(),
        );
        let (account, nanotime, message) = record_in(&tagged("zirc-recorder", &outcome)).unwrap();
        assert_eq!(account.as_deref(), Some("ada-mbp"));
        assert_eq!(nanotime, 1_794_139_200_000_000_000);
        assert_eq!(WakeOutcome::of(&message.event).unwrap().after, Some(40));
        assert!(
            record_in(&tagged("#api", &outcome)).is_none(),
            "a channel's isn't for it"
        );
        let mut plain = tagged("zirc-recorder", &outcome);
        plain.command = "PRIVMSG".into();
        assert!(record_in(&plain).is_none());
    }
}
