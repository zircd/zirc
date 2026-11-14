//! Both changing: a file two or more people or agent sessions have uncommitted changes in
//! at once, as every app shows it (the design lead's `STEP-BOTH-CHANGING.md`). It counts by
//! session, so two agent sessions on one machine count as two. Whether they change the same
//! lines is known only from this machine's zircd, which compares its own worktrees
//! ([`LocalOverlap`]) and a push with a worktree ([`Conflict`]): elsewhere it's never
//! guessed.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use chrono::{DateTime, Utc};
use serde::Serialize;
use ts_rs::TS;
use zirc_proto::{Event, Message};

use super::{Chat, Entry, EntryKind, Icon, Parts, fold};
use crate::presence::{Conflict, LocalOverlap, Member};

/// The kind of the stream's row saying a file is now changed by two at once. Only this
/// client makes it, from what it hears: it's never sent.
pub const BOTH_CHANGING_KIND: &str = "x-zirc/both-changing";

/// One of those changing a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BothChanger {
    /// Their machine's nick.
    pub nick: String,
    /// Their agent session, when it's one; `None` for a machine's whole set, a person's.
    pub session: Option<String>,
    /// What to call them, as [`Chat::title`] would: an agent session by the name it goes by,
    /// `checkout`, or else its id; a person by their name, `Maya Chen`.
    pub name: String,
}

/// A file more than one session is changing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BothChanging {
    /// The path, relative to the repository.
    pub path: String,
    /// Who, each once, sorted by name.
    pub who: Vec<BothChanger>,
    /// Whether two of them change the same or neighbouring lines: only ever from this
    /// machine's zircd ([`Chat::learn_overlap`], [`Chat::learn_conflict`]).
    pub same_lines: bool,
    /// Whether one of them is ours: this machine, or one of our own.
    pub mine: bool,
}

/// How many of what a session shares its card shows, the rest said as `and 2 more`
/// ([`super::more_paths_label`]).
pub const BOTH_CHANGING_SHOWN: usize = 3;

/// What a list's caption says of several files others are changing too: `3 files others are
/// changing`.
#[must_use]
pub fn both_changing_files_label(count: usize) -> String {
    format!("{count} files others are changing")
}

/// What tells one session's working set from another's: its machine and its session, or
/// else its worktree or branch.
type Key = (String, String);

fn key(member: &Member) -> Key {
    let place = member
        .session
        .clone()
        .or_else(|| member.name.clone())
        .or_else(|| member.branch.clone())
        .unwrap_or_default();
    (fold(&member.nick), place)
}

/// What's known of the same lines, and which rows the stream has had.
#[derive(Debug, Default)]
pub(super) struct Known {
    /// By channel and path, the pairs of sets said to change the same lines.
    same_lines: BTreeMap<(String, String), BTreeSet<(Key, Key)>>,
    /// Each channel, path and pair the stream has said is changing it, so it says so once.
    said: HashSet<(String, String, Key, Key)>,
    /// By channel, each set's paths as last heard, to tell who just began changing one.
    paths: BTreeMap<String, BTreeMap<Key, BTreeSet<String>>>,
}

fn pair(a: Key, b: Key) -> (Key, Key) {
    if a <= b { (a, b) } else { (b, a) }
}

/// `others`' names as the line says them: `checkout`, `checkout and rate-limits`,
/// `checkout and 2 others`.
fn names(others: &[String]) -> String {
    match others {
        [] => String::new(),
        [one] => one.clone(),
        [one, two] => format!("{one} and {two}"),
        [one, rest @ ..] => format!("{one} and {} others", rest.len()),
    }
}

/// The verb and the words after the path, from the side of one of those changing it, the
/// others named as `others`: `also changing` … `with checkout`, or `changing the same lines
/// of` … `as checkout`. The one phrase every app says it in: [`Chat::both_changing_line`]
/// and the stream's row use it.
#[must_use]
pub fn both_changing_words(others: &[String], same_lines: bool) -> (String, String) {
    if same_lines {
        (
            "changing the same lines of".to_owned(),
            format!("as {}", names(others)),
        )
    } else {
        (
            "also changing".to_owned(),
            format!("with {}", names(others)),
        )
    }
}

/// The one phrase a file both changing is said in, as a card, a people row's caption and a
/// list draw it: `!` in warning, `also changing`, the path in mono, then `with checkout`,
/// all one line. Ready to draw and send as it is: every app reads this record, the phones
/// through `UniFFI` and web as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct BothChangingLine {
    /// The marker, `!`, drawn in the warning tone and hidden from screen readers.
    pub glyph: String,
    /// The marker and the verb: `! also changing`.
    pub lead: String,
    /// The verb: `also changing`, `changing the same lines of`.
    pub verb: String,
    /// The path, whole on a card, its file's name in a caption; empty for several files.
    pub path: String,
    /// The words after it: `with checkout`, `as checkout`, `with checkout and 2 others`, or
    /// for several files, `3 files others are changing`.
    pub details: String,
}

impl From<Parts> for BothChangingLine {
    fn from(parts: Parts) -> Self {
        Self {
            glyph: parts.icon.glyph().to_owned(),
            lead: parts.lead(),
            path: parts.paths.into_iter().next().unwrap_or_default(),
            verb: parts.verb,
            details: parts.details,
        }
    }
}

impl BothChangingLine {
    /// `path` as the one phrase says it, from the side of one of those changing it, `others`
    /// the rest by name ([`both_changing_words`]): for a line made without a chat to ask.
    #[must_use]
    pub fn said(path: String, others: &[String], same_lines: bool) -> Self {
        let (verb, details) = both_changing_words(others, same_lines);
        let glyph = Icon::Overlap.glyph().to_owned();
        Self {
            lead: format!("{glyph} {verb}"),
            glyph,
            verb,
            path,
            details,
        }
    }
}

/// Whether `changer` is `from`: its session, or, for a person's machine, its nick.
fn is(changer: &BothChanger, from: &str) -> bool {
    changer
        .session
        .as_deref()
        .map_or_else(|| changer.nick.eq_ignore_ascii_case(from), |s| s == from)
}

impl Chat {
    /// The files in `channel` that more than one session is changing: ours first, then
    /// those on the same lines, then by path.
    #[must_use]
    pub fn both_changing(&self, channel: &str) -> Vec<BothChanging> {
        let mut by_path: BTreeMap<String, BTreeMap<Key, BothChanger>> = BTreeMap::new();
        for member in self.presence.members(channel) {
            let changer = self.changer(&member);
            let key = key(&member);
            for path in &member.paths {
                by_path
                    .entry(path.clone())
                    .or_default()
                    .entry(key.clone())
                    .or_insert_with(|| changer.clone());
            }
        }
        let mut both: Vec<BothChanging> = by_path
            .into_iter()
            .filter(|(_, who)| who.len() > 1)
            .map(|(path, who)| {
                let same_lines = self
                    .both
                    .same_lines
                    .get(&(fold(channel), path.clone()))
                    .is_some_and(|pairs| {
                        pairs
                            .iter()
                            .any(|(a, b)| who.contains_key(a) && who.contains_key(b))
                    });
                let mine = who.values().any(|changer| self.is_ours(&changer.nick));
                let mut who: Vec<BothChanger> = who.into_values().collect();
                who.sort_by(|a, b| a.name.cmp(&b.name));
                BothChanging {
                    path,
                    who,
                    same_lines,
                    mine,
                }
            })
            .collect();
        both.sort_by(|a, b| {
            (!a.mine, !a.same_lines, &a.path).cmp(&(!b.mine, !b.same_lines, &b.path))
        });
        both
    }

    /// What `who`, an agent session's id or a person's nick, is changing that others are
    /// too, in every channel: for their card and their row in the people list.
    #[must_use]
    pub fn both_changing_for(&self, who: &str) -> Vec<BothChanging> {
        let channels: BTreeSet<String> = self
            .conversations
            .iter()
            .filter(|c| super::is_channel(&c.name))
            .map(|c| c.name.clone())
            .collect();
        let mut found: Vec<BothChanging> = Vec::new();
        for channel in channels {
            for item in self.both_changing(&channel) {
                if item.who.iter().any(|changer| is(changer, who))
                    && !found.iter().any(|known| known.path == item.path)
                {
                    found.push(item);
                }
            }
        }
        found
    }

    /// `item` as the one phrase every app says it in, from `from`'s side (a session's id or
    /// a person's nick): the marker `!`, `also changing`, the path and `with checkout`; or
    /// `changing the same lines of`, the path and `as checkout`. Joined, it's spaces and no
    /// comma, `! also changing client.ts with checkout`: the mono path sets the names apart.
    /// The path is [`Parts::paths`], drawn mono and never cut.
    #[must_use]
    pub fn both_changing_line(&self, item: &BothChanging, from: &str) -> Parts {
        let me = item.who.iter().find(|changer| is(changer, from));
        let others: Vec<String> = item
            .who
            .iter()
            .filter(|changer| !is(changer, from))
            .map(|changer| changer.name.clone())
            .collect();
        let (verb, details) = both_changing_words(&others, item.same_lines);
        let now = (self.clock)();
        Parts {
            icon: Icon::Overlap,
            actor: me.map_or_else(|| from.to_owned(), |changer| changer.name.clone()),
            session: me.and_then(|changer| changer.session.clone()),
            owner: None,
            actor_detail: None,
            compact: false,
            to: None,
            verb,
            details,
            paths: vec![item.path.clone()],
            links: Vec::new(),
            time: now,
            until: now,
        }
    }

    /// What a session's row in a people list says under it of `items`, what it shares
    /// ([`Chat::both_changing_for`]), from `from`'s side: for one, its line with the file's
    /// name alone, `! also changing client.ts with checkout`; for several, `! also changing
    /// 3 files others are changing`. `None` for none.
    #[must_use]
    pub fn both_changing_caption(&self, items: &[BothChanging], from: &str) -> Option<Parts> {
        let mut parts = self.both_changing_line(items.first()?, from);
        if let [one] = items {
            let name = one.path.rsplit('/').next().unwrap_or(&one.path);
            parts.paths = vec![name.to_owned()];
        } else {
            "also changing".clone_into(&mut parts.verb);
            parts.details = both_changing_files_label(items.len());
            parts.paths = Vec::new();
        }
        Some(parts)
    }

    /// Learns from this machine's zircd that two of its worktrees change the same lines.
    pub fn learn_overlap(&mut self, overlap: &LocalOverlap) {
        let members = self.presence.members(&overlap.channel);
        let place = |worktree: &crate::presence::Worktree| {
            let name = worktree
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned);
            members
                .iter()
                .find(|member| {
                    self.is_ours(&member.nick)
                        && (member.name.is_some() && member.name == name
                            || member.branch.is_some() && member.branch == worktree.branch)
                })
                .map(key)
        };
        let (Some(a), Some(b)) = (place(&overlap.worktrees[0]), place(&overlap.worktrees[1]))
        else {
            return;
        };
        for path in overlap.same_lines.keys() {
            self.both
                .same_lines
                .entry((fold(&overlap.channel), path.clone()))
                .or_default()
                .insert(pair(a.clone(), b.clone()));
        }
    }

    /// Learns from this machine's zircd that a push changes the same lines as one of its
    /// worktrees.
    pub fn learn_conflict(&mut self, conflict: &Conflict) {
        let members = self.presence.members(&conflict.channel);
        let name = conflict
            .worktree
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned);
        let ours = members.iter().find(|member| {
            self.is_ours(&member.nick)
                && (member.name.is_some() && member.name == name
                    || member.branch.is_some() && member.branch == conflict.worktree.branch)
        });
        let theirs = members.iter().find(|member| {
            member.nick.eq_ignore_ascii_case(&conflict.nick)
                && member.branch.as_deref() == Some(conflict.branch.as_str())
        });
        let (Some(ours), Some(theirs)) = (ours, theirs) else {
            return;
        };
        for path in conflict.same_lines.keys() {
            self.both
                .same_lines
                .entry((fold(&conflict.channel), path.clone()))
                .or_default()
                .insert(pair(key(ours), key(theirs)));
        }
    }

    /// Adds to `channel`'s stream a row for each file a pair of sessions has newly begun
    /// changing together, from the one that joined it: once per path and pair, never
    /// edited or removed, at `when`, the time the working set that made it two was said. It's
    /// never fresh or unread: apps show no toast for it.
    pub(super) fn say_both_changing(&mut self, channel: &str, when: DateTime<Utc>) {
        let Some(at) = self.position(channel) else {
            return;
        };
        let members = self.presence.members(channel);
        let before = self.both.paths.remove(&fold(channel)).unwrap_or_default();
        let had = |member: &Member, path: &str| {
            before
                .get(&key(member))
                .is_some_and(|paths| paths.contains(path))
        };
        let mut rows = Vec::new();
        for item in self.both_changing(channel) {
            let sets: Vec<&Member> = members
                .iter()
                .filter(|member| member.paths.contains(&item.path))
                .collect();
            for (i, first) in sets.iter().enumerate() {
                for second in &sets[i + 1..] {
                    let (a, b) = pair(key(first), key(second));
                    if a == b {
                        continue;
                    }
                    let said = (fold(channel), item.path.clone(), a, b);
                    if self.both.said.insert(said) {
                        // From the one that began changing it now, the other already was.
                        let joined = if had(second, &item.path) && !had(first, &item.path) {
                            first
                        } else {
                            second
                        };
                        rows.push((item.clone(), (*joined).clone()));
                    }
                }
            }
        }
        for (item, joined) in rows {
            let mut entry = Self::both_changing_entry(&item, &joined, when);
            entry.owner = self.machine_owner(&joined.nick).map(str::to_owned);
            self.conversations[at].insert(entry, self.options.max_entries);
        }
        let now = members
            .iter()
            .map(|member| (key(member), member.paths.iter().cloned().collect()))
            .collect();
        self.both.paths.insert(fold(channel), now);
    }

    /// The stream's row for `item`, from `joined`, the session that made it two.
    fn both_changing_entry(item: &BothChanging, joined: &Member, time: DateTime<Utc>) -> Entry {
        let from = joined
            .session
            .clone()
            .unwrap_or_else(|| joined.nick.clone());
        let others: Vec<String> = item
            .who
            .iter()
            .filter(|changer| !is(changer, &from))
            .map(|changer| changer.name.clone())
            .collect();
        let (verb, details) = both_changing_words(&others, item.same_lines);
        let mut message = Message::new(
            joined
                .session
                .clone()
                .unwrap_or_else(|| zirc_proto::NO_SESSION.to_owned()),
            Event::Custom {
                kind: BOTH_CHANGING_KIND.to_owned(),
                data: serde_json::json!({
                    "path": item.path,
                    "verb": verb,
                    "details": details,
                }),
                text: format!("{verb} {} {details}", item.path),
            },
        );
        message.name.clone_from(&joined.session_name);
        Entry {
            time,
            id: None,
            from: joined.nick.clone(),
            owner: None,
            kind: EntryKind::Zirc(message),
            history: false,
            mention: false,
            sent_from: None,
            reply_to: None,
            formatted: None,
            edited: false,
        }
    }

    /// Who `member` is, as the line names them.
    fn changer(&self, member: &Member) -> BothChanger {
        let name = match &member.session {
            Some(session) => member
                .session_name
                .clone()
                .unwrap_or_else(|| self.session_label(session)),
            None => match self.machine(&member.nick) {
                Some((owner, _)) => self.owner_name(&owner),
                None => self.shown_name(&member.nick),
            },
        };
        BothChanger {
            nick: member.nick.clone(),
            session: member.session.clone(),
            name,
        }
    }

    /// Whether `nick` is us or one of our machines.
    fn is_ours(&self, nick: &str) -> bool {
        nick.eq_ignore_ascii_case(&self.me) || self.ours(nick)
    }
}

/// A stream row's verb, details and path, when `event` is one [`Chat::say_both_changing`]
/// made.
pub(super) fn row_words(event: &Event) -> Option<(String, String, String)> {
    let Event::Custom { kind, data, .. } = event else {
        return None;
    };
    if kind != BOTH_CHANGING_KIND {
        return None;
    }
    let word = |name: &str| data.get(name)?.as_str().map(str::to_owned);
    Some((word("verb")?, word("details")?, word("path")?))
}
