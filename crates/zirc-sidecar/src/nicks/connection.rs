//! One of the server's own nicks connected, a role's (`crate::roles`): it joins its channels,
//! says what it's told and passes on what it hears as it happens, so nothing reads Ergo's
//! history on a timer. What came while it was away it doesn't catch up on: the role acted on
//! it, or kept it, when it was said.

use tokio::sync::mpsc;
use tracing::warn;
use zirc_irc::Line;
use zirc_irc::multiline::Batches;
use zirc_proto::Message;

use super::{RETRY, RETRY_MOST, WITHIN};

/// What a connection is told to do.
#[derive(Debug)]
pub enum Do {
    /// Say `message` to the channel or nick.
    Say(String, Box<Message>),
}

/// What a connection heard.
pub enum Heard {
    /// `nick` heard `line`, live and whole.
    Live { nick: String, line: Line },
}

/// Keeps `config`'s nick connected in `channels`, doing what comes on `todo`, until nothing
/// more can come, and passing on what it hears to `heard`. A message that couldn't be sent is
/// sent again once connected again.
pub async fn run(
    config: zirc_irc::Config,
    channels: Vec<String>,
    mut todo: mpsc::UnboundedReceiver<Do>,
    heard: mpsc::UnboundedSender<Heard>,
) {
    let nick = config.nick.to_lowercase();
    // What's to be said next, and how many goes it has had.
    let mut pending: Option<(String, Box<Message>, u8)> = None;
    let mut wait = RETRY;
    loop {
        match tokio::time::timeout(WITHIN, zirc_irc::connect(&config)).await {
            Ok(Ok((client, mut incoming))) => {
                wait = RETRY;
                for channel in &channels {
                    let _ = client.join(channel);
                }
                let mut batches = Batches::default();
                loop {
                    if let Some((to, message, goes)) = pending.take() {
                        // One that doesn't fit the server's lines, or that it won't take in
                        // three goes (a line with a NUL in it, say), is let go: kept, it
                        // would hold everything this nick has to say behind it for good.
                        match crate::say::send(&client, &to, &message) {
                            Ok(()) => {}
                            Err(crate::say::Failed::TooLong) => {
                                warn!(nick, "a nick's message was too long to send: dropped");
                            }
                            Err(_) if goes >= 2 => {
                                warn!(nick, "a nick's message couldn't be sent: dropped");
                            }
                            Err(_) => {
                                pending = Some((to, message, goes + 1));
                                break;
                            }
                        }
                        continue;
                    }
                    tokio::select! {
                        next = todo.recv() => match next {
                            None => {
                                let _ = client.quit("done");
                                return;
                            }
                            Some(Do::Say(to, message)) => pending = Some((to, message, 0)),
                        },
                        line = incoming.recv() => {
                            let Some(line) = line else { break };
                            // Nothing yet while it's part of a multiline message still open.
                            if let Some(line) = batches.feed(line) {
                                let _ = heard.send(Heard::Live { nick: nick.clone(), line });
                            }
                        },
                    }
                }
                warn!(nick = config.nick, "a nick's connection ended");
            }
            Ok(Err(zirc_irc::Error::Sasl(_))) => {
                warn!(nick = config.nick, "a nick's login was refused");
            }
            Ok(Err(error)) => warn!(%error, nick = config.nick, "a nick couldn't connect"),
            Err(_) => warn!(nick = config.nick, "a nick's connecting timed out"),
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(RETRY_MOST);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A nick's login on `ergo`, its account registered as the sidecar makes it.
    async fn bot_on(ergo: &zirc_test_ergo::Ergo) -> zirc_irc::Config {
        let config = crate::say::config("127.0.0.1", ergo.port(), None, "token", "bot");
        let passphrase = config.sasl.as_ref().unwrap().password.clone();
        zirc_test_irc::register(ergo, "bot", &passphrase).await;
        config
    }

    #[tokio::test]
    async fn a_nick_hears_its_channels_live_and_says_what_its_told() {
        let ergo = zirc_test_ergo::require_ergo!();
        let config = bot_on(&ergo).await;
        let (ada, mut ada_hears) = zirc_test_irc::person(&ergo, "ada").await;
        zirc_test_irc::joins(&ada, &mut ada_hears, "#ops").await;
        let (todo, doing) = mpsc::unbounded_channel();
        let (hear, mut heard) = mpsc::unbounded_channel();
        let running = tokio::spawn(run(config, vec!["#ops".to_owned()], doing, hear));
        // Once it's in, what ada says reaches it.
        let joined = |l: &Line| {
            l.command == "JOIN" && l.source.as_deref().is_some_and(|s| s.starts_with("bot!"))
        };
        zirc_test_irc::next_matching(&mut ada_hears, joined).await;
        ada.privmsg("#ops", "live").unwrap();
        let line = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let Heard::Live { line, .. } = heard.recv().await.expect("the connection went on");
                if line.command == "PRIVMSG" {
                    return line;
                }
            }
        })
        .await
        .expect("heard in time");
        assert_eq!(line.params[1], "live");
        // It says what it's told.
        let body = "hello".to_owned();
        let said = Message::new("bot", zirc_proto::Event::Text { body });
        todo.send(Do::Say("#ops".into(), Box::new(said))).unwrap();
        let hello = |l: &Line| l.command == "PRIVMSG" && l.params[1].contains("hello");
        zirc_test_irc::next_matching(&mut ada_hears, hello).await;
        drop(todo);
        running.await.unwrap();
    }
}
