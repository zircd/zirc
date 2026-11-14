//! A workflow's nick connected, or the host's: it says what it's told, joins where it's told,
//! and passes on what it hears as it happens, so nothing reads Ergo's history on a timer.
//!
//! Joining a channel, it catches up first: it asks for the channel's history since [`WINDOW`]
//! ago, or all of it for `#workflows` (`CHATHISTORY`), a page at a time. What it hears live
//! meanwhile waits, and comes after it, as heard live (the host counts a message once, by its
//! id). The history goes on as one [`Heard::Caught`], split at the nick's read marker there,
//! so the host acts on what came after it alone; then the marker moves to the last message,
//! and on to each one heard live after that, each move told ([`Heard::Read`]) for the host to
//! keep. A nick with no marker in a channel, as when it's first there, has read up to its
//! joining, and acts on nothing before it. The host keeps the markers, not Ergo, which keeps them only for
//! an always-on account, which a workflow's nick isn't: it would stay in its channels when
//! it's let go.

use std::collections::{BTreeMap, HashSet};

use tokio::sync::mpsc;
use tracing::warn;
use zirc_irc::Line;
use zirc_irc::multiline::Batches;
use zirc_proto::Message;
use zirc_proto::workflow;

use super::{RETRY, RETRY_MOST, Row, WINDOW, WITHIN};
use crate::exporter::{PRIVMSG, Said as Item, TAGMSG};

/// How many messages a page of a channel's history holds.
const PAGE: usize = 500;

/// What a connection is told to do.
#[derive(Debug)]
pub enum Do {
    /// Say `message` in the channel.
    Say(String, Box<Message>),
    /// Join the channel, and catch up there.
    Join(String),
}

/// What a connection heard.
pub enum Heard {
    /// `nick` caught up in `channel` on joining it: its history since [`WINDOW`] ago (or all
    /// of `#workflows`), oldest first: what it had `seen`, up to its read marker, and what it
    /// hadn't, after it. Without a marker it had seen it all.
    Caught {
        nick: String,
        channel: String,
        seen: Vec<Row>,
        unseen: Vec<Row>,
    },
    /// `nick` heard `line`, live.
    Live { nick: String, line: Line },
    /// `nick`'s read marker in `channel` moved to `mark`.
    Read {
        nick: String,
        channel: String,
        mark: Mark,
    },
}

/// How far a nick has read in a channel: the last message it heard there, by its time and id,
/// as the id tells two in the same millisecond apart.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Mark {
    /// When, as the server said it.
    pub at: String,
    /// Its id, empty for a marker set where there was nothing to read.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
}

/// A channel being caught up in.
#[derive(Default)]
struct Catching {
    /// Its read marker when it joined, if it had one.
    marker: Option<Mark>,
    /// The history batch open, by its reference.
    batch: Option<String>,
    /// How many messages the page asked for gave so far.
    paged: usize,
    /// The history, each message with when it was in milliseconds.
    rows: Vec<(i64, Row)>,
    /// When the last row was, as the server said it, to ask for the next page from.
    last: Option<String>,
    /// The last message's mark.
    read_to: Option<Mark>,
    /// What was heard live meanwhile.
    held: Vec<Line>,
}

/// Keeps `config`'s nick connected in `channels`, doing what comes on `todo`, until nothing
/// more can come, and passing on what it hears to `heard`, from its read markers, `read`, by
/// channel. A message that couldn't be sent is sent again once connected again; channels it's
/// told to join are joined again too.
pub async fn run(
    config: zirc_irc::Config,
    mut channels: Vec<String>,
    mut read: BTreeMap<String, Mark>,
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
                let mut joined = Link {
                    nick: nick.clone(),
                    client,
                    heard: heard.clone(),
                    catching: BTreeMap::new(),
                    batches: Batches::default(),
                    seen: HashSet::new(),
                    read: std::mem::take(&mut read),
                };
                loop {
                    if let Some((to, message, goes)) = pending.take() {
                        // One that doesn't fit the server's lines, or that it won't take in
                        // three goes (a line with a NUL in it, say), is let go: kept, it
                        // would hold everything this nick has to say behind it for good.
                        match crate::say::send(&joined.client, &to, &message) {
                            Ok(()) => {}
                            Err(crate::say::Failed::TooLong) => {
                                warn!(nick, "a workflow's message was too long to send: dropped");
                            }
                            Err(_) if goes >= 2 => {
                                warn!(nick, "a workflow's message couldn't be sent: dropped");
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
                                let _ = joined.client.quit("done");
                                return;
                            }
                            Some(Do::Say(to, message)) => pending = Some((to, message, 0)),
                            Some(Do::Join(channel)) => {
                                let _ = joined.client.join(&channel);
                                if !channels.contains(&channel) {
                                    channels.push(channel);
                                }
                            }
                        },
                        line = incoming.recv() => {
                            let Some(line) = line else { break };
                            // Nothing yet while it's part of a multiline message still open.
                            if let Some(whole) = joined.batches.feed(line) {
                                joined.take(whole);
                            }
                        },
                    }
                }
                read = joined.read;
                warn!(nick = config.nick, "a workflow's connection ended");
            }
            Ok(Err(zirc_irc::Error::Sasl(_))) => {
                warn!(nick = config.nick, "a workflow's login was refused");
            }
            Ok(Err(error)) => warn!(%error, nick = config.nick, "a workflow couldn't connect"),
            Err(_) => warn!(nick = config.nick, "a workflow's connecting timed out"),
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(RETRY_MOST);
    }
}

/// One connection's catching up.
struct Link {
    nick: String,
    client: zirc_irc::Client,
    heard: mpsc::UnboundedSender<Heard>,
    catching: BTreeMap<String, Catching>,
    batches: Batches,
    /// The messages caught up on, by id, so one also heard live meanwhile counts once.
    seen: HashSet<String>,
    /// Its read marker in each channel.
    read: BTreeMap<String, Mark>,
}

impl Link {
    /// What the server's `line`, whole, does.
    fn take(&mut self, line: Line) {
        // A history batch's own lines, its joins and parts as much as its messages.
        if let Some(batch) = line.tag("batch").map(str::to_owned)
            && let Some((_, catching)) = self
                .catching
                .iter_mut()
                .find(|(_, c)| c.batch.as_deref() == Some(&batch))
        {
            catching.paged += 1;
            if let Some(time) = line.tag("time") {
                catching.last = Some(time.to_owned());
            }
            if let Some(row) = row_of(&line)
                && let Some(time) = line.tag("time")
                && self.seen.insert(row.msgid.clone())
            {
                catching.read_to = Some(Mark {
                    at: time.to_owned(),
                    id: row.msgid.clone(),
                });
                catching.rows.push((millis(time).unwrap_or_default(), row));
            }
            return;
        }
        let param = |i: usize| line.params.get(i).map(|p| p.to_lowercase());
        let from = source_nick(&line);
        match line.command.as_str() {
            "JOIN" if from.as_deref() == Some(&self.nick) => {
                if let Some(channel) = param(0) {
                    // First there, it has read up to its joining: what's said after it, even
                    // before the history comes, is new.
                    let joined = line.tag("time").map(|at| Mark {
                        at: at.to_owned(),
                        id: String::new(),
                    });
                    let marker = self.read.get(&channel).cloned().or(joined);
                    self.catching.insert(
                        channel.clone(),
                        Catching {
                            marker,
                            ..Catching::default()
                        },
                    );
                    let from = if channel == workflow::CHANNEL {
                        "1970-01-01T00:00:00.000Z".to_owned()
                    } else {
                        time_tag(now().saturating_sub(WINDOW.as_secs()))
                    };
                    self.page(&channel, &from);
                }
                self.forward(line);
            }
            "BATCH" => self.batch(&line),
            "FAIL" if param(0).as_deref() == Some("chathistory") => {
                // No history to be had: it's caught up with what it has.
                let channels: Vec<String> = self
                    .catching
                    .iter()
                    .filter(|(_, c)| c.batch.is_none())
                    .map(|(name, _)| name.clone())
                    .collect();
                for channel in channels {
                    self.caught(&channel);
                }
            }
            _ => {
                if let Some(catching) = param(0).and_then(|c| self.catching.get_mut(&c))
                    && matches!(line.command.as_str(), "PRIVMSG" | "TAGMSG")
                {
                    catching.held.push(line);
                    return;
                }
                self.forward(line);
            }
        }
    }

    /// Passes on `line`, heard live, and moves the marker past it if it's a message.
    fn forward(&mut self, line: Line) {
        let mark = row_of(&line).and_then(|row| {
            let at = line.tag("time")?.to_owned();
            Some((row.channel, Mark { at, id: row.msgid }))
        });
        let _ = self.heard.send(Heard::Live {
            nick: self.nick.clone(),
            line,
        });
        if let Some((channel, mark)) = mark {
            self.mark(&channel, mark);
        }
    }

    fn page(&mut self, channel: &str, from: &str) {
        if let Some(catching) = self.catching.get_mut(channel) {
            catching.paged = 0;
        }
        let page = PAGE.to_string();
        let asked = self.client.send(Line::new(
            "CHATHISTORY",
            ["AFTER", channel, &format!("timestamp={from}"), &page],
        ));
        if asked.is_err() {
            self.caught(channel);
        }
    }

    fn batch(&mut self, line: &Line) {
        let param = |i: usize| line.params.get(i).map(String::as_str).unwrap_or_default();
        let reference = param(0);
        if let Some(opened) = reference.strip_prefix('+') {
            if param(1) == "chathistory"
                && let Some(catching) = self.catching.get_mut(&param(2).to_lowercase())
            {
                catching.batch = Some(opened.to_owned());
            }
            return;
        }
        let Some(closed) = reference.strip_prefix('-') else {
            return;
        };
        let Some((channel, catching)) = self
            .catching
            .iter_mut()
            .find(|(_, c)| c.batch.as_deref() == Some(closed))
        else {
            return;
        };
        let channel = channel.clone();
        catching.batch = None;
        // A whole page: there may be more after it.
        if catching.paged >= PAGE
            && let Some(last) = catching.last.clone()
        {
            return self.page(&channel, &last);
        }
        self.caught(&channel);
    }

    /// Done catching up in `channel`: what it heard goes on, and its marker moves.
    fn caught(&mut self, channel: &str) {
        let Some(catching) = self.catching.remove(channel) else {
            return;
        };
        let Catching {
            marker,
            mut rows,
            held,
            read_to,
            ..
        } = catching;
        // After the marker's message where the history has it, else from its time on (a
        // marker set on joining names no message); all of it seen without one.
        let unseen = match &marker {
            None => Vec::new(),
            Some(mark) => {
                let since = millis(&mark.at).unwrap_or_default();
                let after = rows
                    .iter()
                    .position(|(_, row)| row.msgid == mark.id)
                    .map(|at| at + 1)
                    .or_else(|| rows.iter().position(|(at, _)| *at >= since))
                    .unwrap_or(rows.len());
                rows.split_off(after)
            }
        };
        let seen = rows;
        let rows = |rows: Vec<(i64, Row)>| rows.into_iter().map(|(_, row)| row).collect();
        let _ = self.heard.send(Heard::Caught {
            nick: self.nick.clone(),
            channel: channel.to_owned(),
            seen: rows(seen),
            unseen: rows(unseen),
        });
        let to = read_to.or(marker).unwrap_or_else(|| Mark {
            at: time_tag(now()),
            id: String::new(),
        });
        self.mark(channel, to);
        // Then what was heard meanwhile, which the host counts once if the history had it.
        for line in held {
            self.forward(line);
        }
    }

    /// Moves the marker in `channel` on to `at`, never back.
    fn mark(&mut self, channel: &str, mark: Mark) {
        let on = self
            .read
            .get(channel)
            .is_none_or(|was| was != &mark && millis(&was.at) <= millis(&mark.at));
        if on {
            self.read.insert(channel.to_owned(), mark.clone());
            let _ = self.heard.send(Heard::Read {
                nick: self.nick.clone(),
                channel: channel.to_owned(),
                mark,
            });
        }
    }
}

/// The nick `line` came from.
fn source_nick(line: &Line) -> Option<String> {
    line.source
        .as_deref()
        .and_then(|source| source.split('!').next())
        .map(str::to_lowercase)
}

/// `line` as a row of a channel's history, if it's a message to one with an id and a time.
pub fn row_of(line: &Line) -> Option<Row> {
    let kind = match line.command.as_str() {
        "PRIVMSG" => PRIVMSG,
        "TAGMSG" => TAGMSG,
        _ => return None,
    };
    let channel = line.params.first().filter(|c| c.starts_with('#'))?;
    let tags = line
        .tags
        .iter()
        .filter(|(key, _)| key.starts_with('+'))
        .map(|(key, value)| (key.clone(), value.clone().unwrap_or_default()))
        .collect();
    Some(Row {
        msgid: line.tag("msgid")?.to_owned(),
        channel: channel.to_lowercase(),
        at: seconds(line.tag("time")?)?,
        item: Item {
            kind,
            nick: source_nick(line).unwrap_or_default(),
            account: line.tag("account").unwrap_or_default().to_owned(),
            text: line.params.get(1).cloned().unwrap_or_default(),
            tags,
        },
    })
}

/// A server time, `2026-11-13T12:00:00.000Z`, in unix seconds.
fn seconds(time: &str) -> Option<u64> {
    u64::try_from(millis(time)? / 1000).ok()
}

/// A server time in unix milliseconds.
fn millis(time: &str) -> Option<i64> {
    let at: jiff::Timestamp = time.parse().ok()?;
    Some(at.as_millisecond())
}

/// Unix seconds as a server time.
fn time_tag(seconds: u64) -> String {
    let at = jiff::Timestamp::from_second(i64::try_from(seconds).unwrap_or_default())
        .unwrap_or_default();
    at.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// Now, in unix seconds.
pub fn now() -> u64 {
    u64::try_from(jiff::Timestamp::now().as_second()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_to_a_channel_is_a_row_of_its_history() {
        let line: Line = "@msgid=abc;time=2026-11-13T12:00:00.000Z;account=ada;+draft/react=👍 \
                          :ada!a@h TAGMSG #Ops"
            .parse()
            .unwrap();
        let row = row_of(&line).unwrap();
        assert_eq!((row.msgid.as_str(), row.channel.as_str()), ("abc", "#ops"));
        assert_eq!(row.at, seconds("2026-11-13T12:00:00Z").unwrap());
        assert_eq!(row.item.account, "ada");
        assert_eq!(row.item.tags["+draft/react"], "👍");
        let no_id: Line = "@time=2026-11-13T12:00:00.000Z :ada PRIVMSG #ops :hi"
            .parse()
            .unwrap();
        assert!(row_of(&no_id).is_none());
        assert_eq!(time_tag(row.at), "2026-11-13T12:00:00.000Z");
    }

    /// What `heard` says next that's `what`, within a while, keeping the markers it moves in
    /// `read`, as the host does.
    async fn next(
        heard: &mut mpsc::UnboundedReceiver<Heard>,
        read: &mut BTreeMap<String, Mark>,
        what: impl Fn(&Heard) -> bool,
    ) -> Heard {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let next = heard.recv().await.expect("the connection went on");
                if let Heard::Read { channel, mark, .. } = &next {
                    read.insert(channel.clone(), mark.clone());
                }
                if what(&next) {
                    return next;
                }
            }
        })
        .await
        .expect("heard in time")
    }

    fn texts(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|row| row.item.text.as_str()).collect()
    }

    /// What it caught up on next: what it had seen, and what it hadn't.
    async fn caught(
        heard: &mut mpsc::UnboundedReceiver<Heard>,
        read: &mut BTreeMap<String, Mark>,
    ) -> (Vec<Row>, Vec<Row>) {
        match next(heard, read, |h| matches!(h, Heard::Caught { .. })).await {
            Heard::Caught { seen, unseen, .. } => (seen, unseen),
            _ => unreachable!(),
        }
    }

    /// A workflow's nick's login on `ergo`, its account registered as the sidecar makes it.
    async fn bot_on(ergo: &zirc_test_ergo::Ergo) -> zirc_irc::Config {
        let config = crate::say::config("127.0.0.1", ergo.port(), None, "token", "bot");
        let passphrase = config.sasl.as_ref().unwrap().password.clone();
        zirc_test_irc::register(ergo, "bot", &passphrase).await;
        config
    }

    #[tokio::test]
    async fn a_nick_catches_up_from_its_read_marker_and_hears_the_rest_live() {
        let ergo = zirc_test_ergo::require_ergo!();
        let config = bot_on(&ergo).await;
        let (ada, mut ada_hears) = zirc_test_irc::person(&ergo, "ada").await;
        zirc_test_irc::joins(&ada, &mut ada_hears, "#ops").await;
        ada.privmsg("#ops", "before").unwrap();

        // First there: it had seen everything before, and sets its marker.
        let (todo, doing) = mpsc::unbounded_channel();
        let (hear, mut heard) = mpsc::unbounded_channel();
        let mut read = BTreeMap::new();
        let channels = vec!["#ops".to_owned()];
        let first = tokio::spawn(run(
            config.clone(),
            channels.clone(),
            read.clone(),
            doing,
            hear.clone(),
        ));
        let (seen, unseen) = caught(&mut heard, &mut read).await;
        assert_eq!((texts(&seen), unseen.len()), (vec!["before"], 0));
        ada.privmsg("#ops", "live").unwrap();
        let live = |h: &Heard| matches!(h, Heard::Live { line, .. } if line.command == "PRIVMSG");
        let Heard::Live { line, .. } = next(&mut heard, &mut read, live).await else {
            unreachable!()
        };
        assert_eq!(line.params[1], "live");
        // It says what it's told.
        let body = "hello".to_owned();
        let said = Message::new("bot", zirc_proto::Event::Text { body });
        todo.send(Do::Say("#ops".into(), Box::new(said))).unwrap();
        let hello = |l: &Line| l.command == "PRIVMSG" && l.params[1].contains("hello");
        zirc_test_irc::next_matching(&mut ada_hears, hello).await;

        // Away, then back: what came meanwhile is new, and nothing before it.
        drop(todo);
        first.await.unwrap();
        ada.privmsg("#ops", "meanwhile").unwrap();
        // The server takes her lines in order: once she's in another channel, it has it.
        zirc_test_irc::joins(&ada, &mut ada_hears, "#elsewhere").await;
        // Ergo keeps no marker for it: the host does, and gives it back.
        while let Ok(heard) = heard.try_recv() {
            if let Heard::Read { channel, mark, .. } = heard {
                read.insert(channel, mark);
            }
        }
        let (_todo, doing) = mpsc::unbounded_channel();
        tokio::spawn(run(config, channels, read.clone(), doing, hear));
        let (seen, unseen) = caught(&mut heard, &mut read).await;
        assert_eq!(texts(&seen)[..2], ["before", "live"]);
        // Its own words after what it last heard are new too: the host knows them for its own.
        assert_eq!(texts(&unseen).last(), Some(&"meanwhile"));
        assert!(!texts(&unseen).contains(&"live"));
    }
}
