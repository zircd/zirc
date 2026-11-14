//! What an integration says in a channel (`POST /v1/say` on the account endpoint): zirc.dev's
//! relay for incoming hooks (web's lib/hooks.ts) has verified a delivery and mapped it to a
//! zirc [`Custom`](zirc_proto::Event::Custom) event, and asks the sidecar to post it as the
//! integration's nick, `zirc-<name>`.
//!
//! It also posts what an organisation's CI says with the organisation's CI token (web's
//! lib/ci-tokens.ts), when the relay marks a post as CI's ([`Say::ci`]): as `zirc-ci`, from the
//! session `ci`, in zirc's own CI kinds as well as a team's ([`zirc_proto::ci::may_say`]), with
//! a landing's card made here ([`zirc_proto::ci::landing_card`]) and its text's lines kept.
//!
//! The sidecar is the authority on what may be said: a nick of the form `zirc-<name>`, never a
//! person's or a machine's; a custom kind of a team's own, never zirc's (`x-zirc/…`, which say
//! who is changing what and how CI went); a card that draws (`docs/cards.md`); text with no
//! line breaks or control characters, which would split or frame IRC lines. Its account is made
//! when it's first needed, with a passphrase derived from the endpoint's token, so none is
//! stored. It connects on Ergo's loopback as that account, joins the channel (an invite-only or
//! banned channel refuses it), says it, and leaves. Nothing it says is logged: only who, where
//! and what kind.

use std::time::Duration;

use serde::Deserialize;
use zirc_irc::{Client, Config, Credentials, Incoming};
use zirc_proto::card::Card;
use zirc_proto::{Event, Message};

/// The most a post's text is, in characters.
pub const MAX_TEXT: usize = 400;

/// What to say, as zirc.dev's relay sends it.
#[derive(Debug, Deserialize)]
pub struct Say {
    /// The integration's nick, `zirc-<name>`.
    pub nick: String,
    /// Where: `#channel`.
    pub channel: String,
    /// The event's kind, `x-<team>/<name>`.
    pub kind: String,
    /// What it says for people.
    pub text: String,
    /// What tools and apps read from it: a `card`, say.
    #[serde(default)]
    pub data: serde_json::Value,
    /// Whether it's the organisation's CI saying it, with its CI token: then the nick is
    /// [`zirc_proto::ci::NICK`] and zirc's CI kinds may be said.
    #[serde(default)]
    pub ci: bool,
    /// What CI's post asks of its readers, when CI says; an integration's has none.
    #[serde(default)]
    pub level: Option<zirc_proto::Level>,
}

/// An integration's nick, as zirc-proto says (`zirc-` and a short plain name), and not one of
/// zirc's own ([`zirc_proto::names::RESERVED_INTEGRATION_NICKS`]), which no team's hook posts as.
#[must_use]
pub fn is_integration_nick(nick: &str) -> bool {
    zirc_proto::names::is_integration_nick(nick)
        && !zirc_proto::names::RESERVED_INTEGRATION_NICKS.contains(&nick)
}

/// `text` as one line: control characters, which would frame or split an IRC line, become
/// spaces.
fn plain(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` with its lines kept, each one clean ([`plain`]), and empty lines dropped.
fn lines(text: &str) -> String {
    text.lines()
        .map(plain)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

impl Say {
    /// The message to send, or why it can't be said.
    ///
    /// # Errors
    ///
    /// What's wrong with it, in words for the relay's log.
    pub fn message(&self) -> Result<Message, String> {
        if self.ci {
            if self.nick != zirc_proto::ci::NICK {
                return Err(format!("CI posts as {}", zirc_proto::ci::NICK));
            }
        } else if !is_integration_nick(&self.nick) {
            return Err("an integration posts as zirc-<name>".into());
        }
        let channel = &self.channel;
        if !channel.starts_with('#')
            || channel.len() > 64
            || channel
                .chars()
                .any(|c| c.is_control() || c == ' ' || c == ',')
        {
            return Err("a channel is #name".into());
        }
        if let Some(problem) = zirc_proto::check_kind(&self.kind) {
            return Err(problem.into());
        }
        if self.ci {
            if !zirc_proto::ci::may_say(&self.kind) {
                return Err(
                    "CI says a landing, a deploy, a check, a state or a team's own kind".into(),
                );
            }
        } else if self.kind.starts_with("x-zirc/") {
            return Err("x-zirc/ kinds are zirc's own".into());
        }
        if let Some(card) = self.data.get("card") {
            let card: Card =
                serde_json::from_value(card.clone()).map_err(|e| format!("the card: {e}"))?;
            card.validate().map_err(|e| format!("the card: {e}"))?;
        }
        let (text, most) = if self.ci {
            (lines(&self.text), zirc_proto::ci::MAX_TEXT)
        } else {
            (plain(&self.text), MAX_TEXT)
        };
        if text.is_empty() || text.chars().count() > most {
            return Err(format!("the text is 1 to {most} characters"));
        }
        let data = if self.data.is_null() {
            serde_json::json!({})
        } else {
            self.data.clone()
        };
        // A landing's card is made here from what CI says of it, as every landing's is.
        // Without its buttons: a press goes to the integration registered as the card's sender,
        // and nothing answers for an organisation's CI yet.
        let data = match zirc_proto::ci::landing_card(&data, &text) {
            Some(mut card) if self.ci && data.get("card").is_none() => {
                card.actions.clear();
                card.into_data(data)
            }
            _ => data,
        };
        let session = if self.ci {
            zirc_proto::ci::SESSION
        } else {
            &self.nick
        };
        let level = if self.ci {
            zirc_proto::ci::level(&self.kind, &data, self.level)
        } else {
            None
        };
        let mut message = Message::new(
            session,
            Event::Custom {
                kind: self.kind.clone(),
                data,
                text,
            },
        );
        message.app = zirc_proto::app_label(self.nick.trim_start_matches("zirc-"));
        message.level = level;
        Ok(message)
    }
}

/// How an integration's account logs in: derived from the endpoint's token, so it's never
/// stored and is the same each time.
#[must_use]
pub fn passphrase(secret: &str, nick: &str) -> String {
    crate::hmac_hex(secret, format!("say:{nick}").as_bytes())
}

/// Why posting failed, for the log: never what was said.
#[derive(Debug, thiserror::Error)]
pub enum Failed {
    /// Couldn't connect: the network, a timeout, the server's refusal.
    #[error("couldn't connect as the integration: {0}")]
    Connect(String),
    /// The server refused the account's login: its passphrase isn't the derived one. The only
    /// failure the sidecar answers by setting the passphrase again.
    #[error("the integration's login was refused: {0}")]
    Auth(String),
    /// The server wouldn't let it into the channel, or it took too long.
    #[error("couldn't join the channel: {0}")]
    Join(String),
    /// The message doesn't fit what the server takes.
    #[error("the message is too long for the server")]
    TooLong,
    /// The connection broke.
    #[error("sending failed: {0}")]
    Send(String),
}

const WITHIN: Duration = Duration::from_secs(8);

/// Connects as `config`'s account, joins the channel, sends `message` and leaves.
///
/// # Errors
///
/// [`Failed`], saying where it went wrong.
pub async fn post(config: &Config, channel: &str, message: &Message) -> Result<(), Failed> {
    let (client, mut incoming) = tokio::time::timeout(WITHIN, zirc_irc::connect(config))
        .await
        .map_err(|_| Failed::Connect("timed out".into()))?
        .map_err(|e| match e {
            zirc_irc::Error::Sasl(why) => Failed::Auth(why),
            other => Failed::Connect(other.to_string()),
        })?;
    let result = deliver(&client, &mut incoming, channel, message).await;
    let _ = client.quit("done");
    // Until the server has taken what was queued and closed, as quitting first would drop it.
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while incoming.recv().await.is_some() {}
    })
    .await;
    result
}

async fn deliver(
    client: &Client,
    incoming: &mut Incoming,
    channel: &str,
    message: &Message,
) -> Result<(), Failed> {
    client
        .join(channel)
        .map_err(|e| Failed::Join(e.to_string()))?;
    tokio::time::timeout(WITHIN, async {
        while let Some(line) = incoming.recv().await {
            if line.command == "JOIN" {
                return Ok(());
            }
            // 4xx numerics: no such channel, banned, invite only, full, and so on.
            // But 422, which a server without a message of the day says to everyone connecting.
            if line.command.len() == 3 && line.command.starts_with('4') && line.command != "422" {
                return Err(Failed::Join(format!("the server said {}", line.command)));
            }
        }
        Err(Failed::Join("the connection ended".into()))
    })
    .await
    .map_err(|_| Failed::Join("timed out".into()))??;
    send(client, channel, message)
}

/// Sends `message` to `channel` on one line, or as a multiline message when it's longer.
///
/// # Errors
///
/// [`Failed::TooLong`] when it doesn't fit even so; [`Failed::Send`] when the connection broke.
pub fn send(client: &Client, channel: &str, message: &Message) -> Result<(), Failed> {
    let sent = if let Ok(body) = zirc_proto::encode(message) {
        client.privmsg(channel, &body)
    } else {
        let limits = client.multiline().ok_or(Failed::TooLong)?;
        let body =
            zirc_proto::encode_up_to(message, limits.max_bytes).map_err(|_| Failed::TooLong)?;
        client.privmsg_multiline(channel, &body)
    };
    sent.map_err(|e| Failed::Send(e.to_string()))
}

/// The login of `nick`'s account, with the passphrase derived from `secret`.
#[must_use]
pub fn config(host: &str, port: u16, gate: Option<&str>, secret: &str, nick: &str) -> Config {
    let mut config = Config::new(host, port, nick);
    config.tls = false;
    config.server_password = gate.map(str::to_owned);
    config.sasl = Some(Credentials {
        account: nick.to_owned(),
        password: passphrase(secret, nick),
    });
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn say(kind: &str, text: &str, data: serde_json::Value) -> Say {
        Say {
            nick: "zirc-github".into(),
            channel: "#ops".into(),
            kind: kind.into(),
            text: text.into(),
            data,
            ci: false,
            level: None,
        }
    }

    fn ci(kind: &str, text: &str, data: serde_json::Value) -> Say {
        Say {
            nick: zirc_proto::ci::NICK.into(),
            ci: true,
            ..say(kind, text, data)
        }
    }

    #[test]
    fn ci_says_a_landing_as_zirc_ci_with_its_card_and_lines() {
        let data = json!({"landing": "web/7", "build": "7", "state": "failed", "session": "cc-1"});
        let text = "cc-1: build 7 failed\r\n- failed: `//web:e2e`\u{1}";
        let message = ci("x-zirc/landing", text, data).message().unwrap();
        assert_eq!(message.session, "ci");
        assert_eq!(message.app.as_deref(), Some("ci"));
        let Event::Custom { data, text, .. } = &message.event else {
            panic!("not a custom event")
        };
        assert_eq!(text, "cc-1: build 7 failed\n- failed: `//web:e2e`");
        assert_eq!(data["card"]["id"], "web/7");
        assert_eq!(data["card"]["state"], "failed");
        assert_eq!(data["card"]["about"], "cc-1");
        assert!(data["card"].get("actions").is_none_or(|a| a == &json!([])));
    }

    #[test]
    fn ci_s_kinds_are_ci_s_alone() {
        // Only as zirc-ci, and only marked as CI's: a hook can't pass for it.
        assert!(say("x-zirc/landing", "x", json!({})).message().is_err());
        let mut as_hook = ci("x-zirc/landing", "x", json!({}));
        as_hook.nick = "zirc-github".into();
        assert!(as_hook.message().is_err());
        let mut unmarked = ci("x-acme/deploy", "x", json!({}));
        unmarked.ci = false;
        assert!(unmarked.message().is_err(), "zirc-ci is reserved");
        // zirc's other kinds stay zirc's.
        assert!(ci("x-zirc/session", "x", json!({})).message().is_err());
        assert!(ci("x-zirc/decision", "x", json!({})).message().is_err());
        assert!(ci("x-acme/deploy", "x", json!({})).message().is_ok());
    }

    #[test]
    fn a_state_ci_says_is_at_its_level() {
        let broken = json!({"name": "master", "level": "broken"});
        let message = ci("x-zirc/state", "#web master is broken", broken)
            .message()
            .unwrap();
        assert_eq!(message.level, Some(zirc_proto::Level::Error));
    }

    #[test]
    fn only_integrations_have_the_nick() {
        for ok in ["zirc-github", "zirc-a", "zirc-ci-2"] {
            assert!(is_integration_nick(ok), "{ok}");
        }
        for bad in [
            "zirc",
            "zirc-",
            "zirc--x",
            "ada",
            "ada-mbp",
            "zirc-Github",
            "zirc-a_b",
            "zirc-aaaaaaaaaaaaaaaaaaaaaaaaaa",
            "zirc-alerts",
            "zirc-ci",
        ] {
            assert!(!is_integration_nick(bad), "{bad}");
        }
    }

    #[test]
    fn a_custom_event_with_a_card_is_sent() {
        let card = json!({"card": {"id": "a/1", "title": "Deploy 7", "state": "passed"}});
        let message = say("x-hook/event", "Deployed 7", card).message().unwrap();
        assert_eq!(message.app.as_deref(), Some("github"));
        assert!(matches!(message.event, Event::Custom { ref kind, .. } if kind == "x-hook/event"));
    }

    #[test]
    fn zirc_s_own_kinds_and_non_kinds_are_refused() {
        assert!(say("x-zirc/landing", "x", json!({})).message().is_err());
        assert!(say("x-zirc/state", "x", json!({})).message().is_err());
        assert!(say("landing", "x", json!({})).message().is_err());
        assert!(say("intent", "x", json!({})).message().is_err());
    }

    #[test]
    fn a_card_that_wont_draw_is_refused() {
        let no_title = json!({"card": {"id": "a", "title": ""}});
        assert!(say("x-a/b", "x", no_title).message().is_err());
        let http = json!({"card": {"id": "a", "title": "t", "links": [{"label": "x", "url": "http://x"}]}});
        assert!(say("x-a/b", "x", http).message().is_err());
    }

    #[test]
    fn text_is_one_clean_line() {
        let message = say("x-a/b", "a\r\nPRIVMSG #x :hi\u{1}\u{0}b", json!({}))
            .message()
            .unwrap();
        let Event::Custom { text, .. } = message.event else {
            panic!()
        };
        assert_eq!(text, "a PRIVMSG #x :hi b");
        assert!(say("x-a/b", "  \n ", json!({})).message().is_err());
        assert!(
            say("x-a/b", &"x".repeat(MAX_TEXT + 1), json!({}))
                .message()
                .is_err()
        );
    }

    #[test]
    fn channels_are_names() {
        for bad in [
            "ops",
            "#a b",
            "#a,#b",
            "#a\u{7}",
            &format!("#{}", "x".repeat(70)),
        ] {
            let mut s = say("x-a/b", "x", json!({}));
            s.channel = bad.into();
            assert!(s.message().is_err(), "{bad}");
        }
    }

    #[test]
    fn the_passphrase_is_stable_and_per_nick() {
        assert_eq!(passphrase("s", "zirc-a"), passphrase("s", "zirc-a"));
        assert_ne!(passphrase("s", "zirc-a"), passphrase("s", "zirc-b"));
        assert_ne!(passphrase("s", "zirc-a"), passphrase("t", "zirc-a"));
    }
}
