//! What was said in this machine's channels while zircd wasn't there to hear it: after its
//! connection to the server dropped, or while it was stopped, as for an update. A machine's
//! account isn't always-on, so the server keeps nothing for it meanwhile, but every
//! channel's history has it all.
//!
//! As zircd joins each channel again it asks for what was said there after the newest
//! message it heard (`CHATHISTORY AFTER`), and tells watchers each message as it would
//! have, live: what concerns a session wakes it, or waits for its next watch, and a landing
//! line ends the waits on it. What came directly to the machine in the gap from its owner,
//! or from a role a session here holds, is asked of each such conversation too and told as
//! it would have been: a server keeps a direct message it took for a connection that had
//! silently gone. Reactions are told again too, in their place among the
//! messages: a 👍 can be a person's answer to a session's `zirc request`. Working sets,
//! joins and parts are how things stood then, and they're said anew as machines rejoin, so
//! none of them changes what this machine knows now. A message or reaction heard live as
//! well, in the moments before the history comes, is told once ([`super::Heard`]).
//!
//! Chat goes stale, so only a short gap is replayed ([`REPLAYED`]): a dropped connection or
//! a restart. After a longer one, such as a laptop asleep overnight, the sessions waiting on
//! something are told that zircd was away and what it may have missed, rather than woken
//! by every line of it. The one thing still read after a long gap is the owner's answers to
//! this machine's open requests ([`Daemon::ask_answers_missed`]): a 👍 given from a phone
//! while a laptop slept went nowhere, its request left open and nobody told (a user's
//! review, 22 Nov 2026).

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use zirc_core::ipc::{Delivery, Response};
use zirc_core::paths;
use zirc_core::project::ORG_CHANNEL;
use zirc_irc::Line;
use zirc_proto::{Event, Message};

use super::{Daemon, WATCHER_SESSION, heard, now, reaction, read_saved, vouch};

/// The longest gap whose messages are told again.
pub(super) const REPLAYED: Duration = Duration::from_mins(10);

/// How long a connection can have been gone before zircd notices ([`zirc_irc::NOTICED_WITHIN`]),
/// with half a minute to spare. What was said meanwhile went to a connection no longer
/// there, so where nothing newer was heard, what's missed is asked from this long before
/// zircd knew it was away; what was heard already is told once.
const UNNOTICED: Duration = Duration::from_secs(zirc_irc::NOTICED_WITHIN.as_secs() + 30);

/// Where to ask what was missed from when nothing in a conversation was heard: from
/// [`UNNOTICED`] before zircd knew it was away `since`, as a time tag.
fn missed_from(since: u64) -> String {
    zirc_core::time::tag(since.saturating_sub(UNNOTICED.as_secs()))
}

/// How long after connecting a channel joined still has its gap asked for: past the joins
/// that follow connecting, a channel joined is a new one, whose past isn't news.
const JOINING: Duration = Duration::from_mins(1);

/// How often the time zircd was last connected is saved, which a restart counts its gap
/// from.
const SAVE_EVERY: Duration = Duration::from_secs(30);

/// The most messages asked for at once: Ergo's `chathistory-maxmessages` by default.
const PAGE: usize = 1000;

/// What a restart needs to ask for what it missed, kept across restarts.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct Saved {
    /// When zircd was last connected, in seconds since the Unix epoch.
    alive: Option<u64>,
    /// By channel, or by the nick of whom a direct message came from, folded to lower case:
    /// the server's time tag of the newest message heard there.
    newest: BTreeMap<String, String>,
}

/// Where replaying stands.
#[derive(Debug, Default)]
pub(super) struct Replay {
    saved: Saved,
    /// Whether `newest` changed since it was saved.
    changed: bool,
    /// When `alive` was last saved.
    saved_at: Option<Instant>,
    /// Since when zircd has been away from the server, while it is, in seconds since the
    /// Unix epoch.
    away_since: Option<u64>,
    /// While joining after a gap short enough to replay: until when a channel joined is
    /// asked for its gap, since when zircd was away, and the channels asked already.
    joining: Option<Joining>,
    /// After a gap too long to replay: until when `#decisions`, once joined, is asked for
    /// the answers given meanwhile to this machine's open requests, and since when zircd
    /// was away, in seconds since the Unix epoch.
    answers: Option<(Instant, u64)>,
    /// The history asked for and not yet all in.
    asked: Vec<Asked>,
}

/// Joining again after a gap. What's done about it waits for the first channel joined:
/// zircd connects while it's still restoring what it saved, such as its sessions and what
/// they wait on, and takes in what the server says only once it's done.
#[derive(Debug)]
struct Joining {
    /// What the gap calls for.
    gap: Gap,
    /// Until when a channel joined is asked for its gap.
    until: Instant,
    /// Since when zircd was away, in seconds since the Unix epoch.
    since: u64,
    /// The channels asked already, folded to lower case.
    asked: BTreeSet<String>,
}

/// One channel's history, asked for.
#[derive(Debug)]
struct Asked {
    channel: String,
    /// The server's name for the batch it comes in, once it starts.
    batch: Option<String>,
    lines: Vec<Line>,
    /// Whether only the answers to this machine's open requests are taken from it, after a
    /// gap too long to tell the rest of.
    answers_only: bool,
    /// Whether it's a direct conversation with this machine, `channel` the nick it's with.
    direct: bool,
}

/// What to do on connecting after zircd was away from the server for `away` seconds, or
/// for an unknown while when `None`, as on its first start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Gap {
    /// Nothing to ask for.
    None,
    /// Ask each channel for what it missed.
    Replay,
    /// Too long ago to tell: say so to whoever waits on something.
    TooLong(u64),
}

impl Gap {
    pub(super) fn of(away: Option<u64>) -> Self {
        match away {
            None => Self::None,
            Some(away) if away <= REPLAYED.as_secs() => Self::Replay,
            Some(away) => Self::TooLong(away),
        }
    }
}

/// How long `seconds` is, as `zirc catchup --since` takes it: rounded up to a minute, or
/// to an hour past one.
pub(super) fn span(seconds: u64) -> String {
    if seconds < 3600 {
        format!("{}m", seconds.div_ceil(60).max(1))
    } else {
        format!("{}h", seconds.div_ceil(3600))
    }
}

impl Daemon {
    /// Takes in what was saved before zircd last stopped: it has been away since it was
    /// last connected.
    pub(super) fn restore_replay(&mut self) {
        if let Some(saved) = read_saved::<Saved>(&paths::replay(&self.org.state)) {
            self.replay.away_since = saved.alive;
            self.replay.saved = saved;
        }
    }

    /// Saves where replaying would start from, for after a restart.
    pub(super) fn save_replay(&mut self) {
        if self.client.is_some() {
            self.replay.saved.alive = Some(now());
        }
        let json = serde_json::to_string(&self.replay.saved).expect("replay serializes");
        if let Err(err) = paths::write_private(&paths::replay(&self.org.state), &json) {
            tracing::warn!("couldn't save where to replay from: {err}");
        }
        self.replay.changed = false;
        self.replay.saved_at = Some(self.clock.now());
    }

    /// Saves the newest messages heard once they've changed, and the time zircd was last
    /// connected now and then.
    pub(super) fn replay_tick(&mut self) {
        let due = self
            .replay
            .saved_at
            .is_none_or(|at| self.clock.now().duration_since(at) >= SAVE_EVERY);
        if self.replay.changed || (due && self.client.is_some()) {
            self.save_replay();
        }
    }

    /// Notes `line`, a message or reaction in `channel`, as the newest heard there, if it is.
    pub(super) fn heard_in(&mut self, channel: &str, line: &Line) {
        let Some(time) = line.tag("time") else { return };
        let newest = self
            .replay
            .saved
            .newest
            .entry(channel.to_ascii_lowercase())
            .or_default();
        if zirc_core::time::millis_at(time) > zirc_core::time::millis_at(newest) {
            time.clone_into(newest);
            self.replay.changed = true;
        }
    }

    /// The connection dropped: zircd's away from now, and what it asked for won't come.
    pub(super) fn replay_disconnected(&mut self) {
        if self.replay.away_since.is_none() {
            self.replay.away_since = Some(now());
        }
        self.replay.joining = None;
        self.replay.answers = None;
        self.replay.asked.clear();
    }

    /// Connected again: each channel joined next is asked for what it missed, if the gap
    /// is short enough; after a longer one, the sessions waiting on something are told.
    pub(super) fn replay_connected(&mut self) {
        let since = self.replay.away_since.take();
        let away = since.map(|since| now().saturating_sub(since));
        let gap = Gap::of(away);
        self.replay.answers = match (gap, since) {
            (Gap::TooLong(_), Some(since)) => Some((self.clock.now() + JOINING, since)),
            _ => None,
        };
        self.replay.joining = (gap != Gap::None).then(|| Joining {
            gap,
            until: self.clock.now() + JOINING,
            since: since.unwrap_or_else(now),
            asked: BTreeSet::new(),
        });
        self.save_replay();
    }

    /// Keeps what concerns this machine's sessions until they watch, so what's replayed
    /// before a session's watch is back, as after a restart, still reaches it.
    fn keep_for_sessions(&mut self) {
        let sessions: Vec<String> = self
            .repos
            .values()
            .flat_map(|watched| watched.sessions.names().map(str::to_owned))
            .collect();
        for session in sessions {
            self.missed.entry(session).or_default();
        }
    }

    /// Tells each session waiting on something that zircd was away for `away` seconds, so
    /// what it waits on may have happened unheard.
    fn tell_away(&mut self, away: u64) {
        let me = self
            .client
            .as_ref()
            .map(|client| client.nick().to_owned())
            .unwrap_or_default();
        let since = span(away);
        for (session, on) in self.awaits.waiting() {
            let body = format!(
                "{session}: zircd was away from the server for {since}, so {} may have \
                 happened unheard: `zirc catchup --since {since}` shows what was said",
                on.join(", ")
            );
            self.tell_watchers(&Response::Delivery(Delivery::new(
                ORG_CHANNEL,
                me.clone(),
                Message::new(WATCHER_SESSION, Event::Text { body }),
            )));
        }
    }

    /// This machine joined `channel`: if it's joining again after a short gap, asks the
    /// server what was said there after the newest message heard, or after zircd was last
    /// connected.
    pub(super) fn replay_joined(&mut self, channel: &str) {
        let at = self.clock.now();
        if channel.eq_ignore_ascii_case(zirc_proto::request::CHANNEL) {
            self.ask_answers_missed();
        }
        let Some(joining) = &mut self.replay.joining else {
            return;
        };
        if at > joining.until {
            self.replay.joining = None;
            return;
        }
        let first = joining.asked.is_empty();
        let folded = channel.to_ascii_lowercase();
        if !joining.asked.insert(folded.clone()) {
            return;
        }
        let since = joining.since;
        if first {
            self.keep_for_sessions();
        }
        if let Gap::TooLong(away) = self.replay.joining.as_ref().map_or(Gap::None, |j| j.gap) {
            tracing::info!("away from the server for {away}s: too long ago to replay");
            self.replay.joining = None;
            self.tell_away(away);
            return;
        }
        let Some(client) = &self.client else { return };
        if !client.has_cap("draft/chathistory") || !client.has_cap("batch") {
            return;
        }
        if first {
            tracing::info!(
                "away from the server for {}s: asking what was said",
                now().saturating_sub(since)
            );
            self.ask_directs_missed(since);
        }
        let Some(client) = &self.client else { return };
        let after = self
            .replay
            .saved
            .newest
            .get(&folded)
            .cloned()
            .unwrap_or_else(|| missed_from(since));
        if client
            .send(after_line(channel, &format!("timestamp={after}")))
            .is_ok()
        {
            self.replay.asked.push(Asked {
                channel: channel.to_owned(),
                batch: None,
                lines: Vec::new(),
                answers_only: false,
                direct: false,
            });
        }
    }

    /// Asks what came to this machine directly while it was away, since `since`, from whom
    /// it matters: its owner, and each role a session here holds, whose wakes come so.
    fn ask_directs_missed(&mut self, since: u64) {
        let mut nicks: BTreeSet<String> = self
            .roles_held()
            .into_iter()
            .map(|(_, role)| role.to_ascii_lowercase())
            .collect();
        nicks.extend(self.owner.as_deref().map(str::to_ascii_lowercase));
        let Some(client) = &self.client else { return };
        for nick in nicks {
            let after = self
                .replay
                .saved
                .newest
                .get(&nick)
                .cloned()
                .unwrap_or_else(|| missed_from(since));
            if client
                .send(after_line(&nick, &format!("timestamp={after}")))
                .is_ok()
            {
                self.replay.asked.push(Asked {
                    channel: nick,
                    batch: None,
                    lines: Vec::new(),
                    answers_only: false,
                    direct: true,
                });
            }
        }
    }

    /// `#decisions` is joined after a gap too long to replay: asks what was said there
    /// meanwhile, for the owner's answers to this machine's open requests alone
    /// ([`Daemon::tell_replayed`]). Nothing else of it is told, and nothing is asked with
    /// no request open.
    fn ask_answers_missed(&mut self) {
        let Some((until, since)) = self.replay.answers.take() else {
            return;
        };
        if self.clock.now() > until || !self.requests.any_open() {
            return;
        }
        let Some(client) = &self.client else { return };
        if !client.has_cap("draft/chathistory") || !client.has_cap("batch") {
            return;
        }
        let channel = zirc_proto::request::CHANNEL;
        let after = self
            .replay
            .saved
            .newest
            .get(channel)
            .cloned()
            .unwrap_or_else(|| missed_from(since));
        tracing::info!("asking for the answers given to open requests while away");
        if client
            .send(after_line(channel, &format!("timestamp={after}")))
            .is_ok()
        {
            self.replay.asked.push(Asked {
                channel: channel.to_owned(),
                batch: None,
                lines: Vec::new(),
                answers_only: true,
                direct: false,
            });
        }
    }

    /// Takes `line` if it's history asked for here: its batch starting or ending, a line of
    /// it, or the server refusing it. Whether it was.
    pub(super) fn replayed(&mut self, line: &Line) -> bool {
        if self.replay.asked.is_empty() {
            return false;
        }
        let param = |i: usize| line.params.get(i).map_or("", String::as_str);
        match line.command.as_str() {
            "BATCH" => {
                if let Some(reference) = param(0).strip_prefix('+') {
                    if param(1) != "chathistory" {
                        return false;
                    }
                    let Some(asked) = self.replay.asked.iter_mut().find(|asked| {
                        asked.batch.is_none() && asked.channel.eq_ignore_ascii_case(param(2))
                    }) else {
                        return false;
                    };
                    asked.batch = Some(reference.to_owned());
                    true
                } else if let Some(reference) = param(0).strip_prefix('-') {
                    let Some(at) = self
                        .replay
                        .asked
                        .iter()
                        .position(|asked| asked.batch.as_deref() == Some(reference))
                    else {
                        return false;
                    };
                    let asked = self.replay.asked.remove(at);
                    self.tell_replayed(asked);
                    true
                } else {
                    false
                }
            }
            "FAIL" if param(0) == "CHATHISTORY" => {
                let Some(at) = self.replay.asked.iter().position(|asked| {
                    asked.batch.is_none()
                        && line
                            .params
                            .iter()
                            .any(|p| p.eq_ignore_ascii_case(&asked.channel))
                }) else {
                    return false;
                };
                let asked = self.replay.asked.remove(at);
                tracing::warn!(
                    "the server wouldn't say what was said in {}: {}",
                    asked.channel,
                    line.params.last().map_or("", String::as_str)
                );
                true
            }
            _ => {
                let Some(batch) = line.tag("batch") else {
                    return false;
                };
                let Some(asked) = self
                    .replay
                    .asked
                    .iter_mut()
                    .find(|asked| asked.batch.as_deref() == Some(batch))
                else {
                    return false;
                };
                asked.lines.push(line.clone());
                true
            }
        }
    }

    /// Tells watchers the messages of a page of `asked`'s history, oldest first, and asks
    /// for the page after it if it was full.
    fn tell_replayed(&mut self, asked: Asked) {
        let Asked {
            channel,
            lines,
            answers_only,
            direct,
            ..
        } = asked;
        let full = lines.len() >= PAGE;
        let last = lines
            .last()
            .and_then(|line| line.tag("msgid"))
            .map(str::to_owned);
        let me = self
            .client
            .as_ref()
            .map(|client| client.nick().to_owned())
            .unwrap_or_default();
        let mut told = 0;
        for line in &lines {
            // A working set (a TAGMSG) is how things stood: said anew as machines rejoin.
            if zirc_core::presence::as_privmsg(line).is_some()
                || zirc_core::presence::old_working(line)
            {
                continue;
            }
            let reacted = line.command == "TAGMSG"
                && (line.tag("+draft/react").is_some() || line.tag("+draft/unreact").is_some());
            if line.command != "PRIVMSG" && !reacted {
                continue;
            }
            let from = line
                .source
                .as_deref()
                .and_then(|source| source.split('!').next())
                .unwrap_or_default();
            // This machine said nothing while it was away, but its own request from just
            // before, whose echo it never heard, is the one its owner's answer is on.
            // A direct message is told as one heard live, which says whom it's for.
            if direct {
                let body = line.params.get(1).filter(|_| line.command == "PRIVMSG");
                if from != me
                    && let Some(body) = body
                    && line.tag("msgid").is_none_or(|id| self.heard.first_time(id))
                {
                    told += 1;
                    self.heard_direct(line, from, body);
                }
                continue;
            }
            if from == me {
                if let Some(id) = line.tag("msgid")
                    && let Some((delivery, _)) = heard(&channel, line)
                {
                    self.request_found(id, &delivery);
                }
                continue;
            }
            if let Some(id) = line.tag("msgid")
                && !self.heard.first_time(id)
            {
                continue;
            }
            self.heard_in(&channel, line);
            if answers_only {
                if reacted && let Some(reaction) = reaction(&channel, from, line) {
                    self.request_reacted(&reaction);
                }
                continue;
            }
            told += 1;
            if reacted {
                self.reacted(&channel, from, line);
                continue;
            }
            let Some((mut delivery, zirc)) = heard(&channel, line) else {
                continue;
            };
            if zirc && matches!(delivery.message.event, Event::Working { .. }) {
                continue;
            }
            vouch(&mut delivery, self.zirc_dev);
            self.remember(&delivery);
            self.tell_watchers(&Response::Delivery(delivery));
        }
        if told > 0 {
            tracing::info!("{told} said or reacted in {channel} while away");
        }
        if full
            && let Some(last) = last
            && let Some(client) = &self.client
            && client
                .send(after_line(&channel, &format!("msgid={last}")))
                .is_ok()
        {
            self.replay.asked.push(Asked {
                channel,
                batch: None,
                lines: Vec::new(),
                answers_only,
                direct,
            });
        }
    }
}

/// `CHATHISTORY AFTER`, a page of `channel`'s history after `anchor`.
fn after_line(channel: &str, anchor: &str) -> Line {
    Line::new("CHATHISTORY", ["AFTER", channel, anchor, &PAGE.to_string()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replays_a_short_gap_and_not_a_long_one() {
        assert_eq!(Gap::of(None), Gap::None);
        assert_eq!(Gap::of(Some(0)), Gap::Replay);
        assert_eq!(Gap::of(Some(600)), Gap::Replay);
        assert_eq!(Gap::of(Some(601)), Gap::TooLong(601));
    }

    #[test]
    fn says_a_gap_as_catchup_takes_it() {
        assert_eq!(span(5), "1m");
        assert_eq!(span(601), "11m");
        assert_eq!(span(3600 * 3 + 12 * 60), "4h");
    }
}
