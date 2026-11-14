//! How people show: the name and picture their zirc.dev profile gives them in the
//! organisation, the same in every app. The organisation's server says them live, as
//! `draft/metadata-2` keys on each account ([`NAME_KEY`], [`AVATAR_KEY`]); zirc.dev's
//! `/api/profiles` says them for those not yet heard. Without either, a person shows as their
//! nick and their initials.

use std::collections::HashMap;

use zirc_proto::profile::Profiles;

use super::{Chat, conversation::fold};

/// The metadata key the server says a person's name under.
pub const NAME_KEY: &str = "display-name";

/// The metadata key the server says a person's picture under: an `https` address, versioned,
/// absent for initials.
pub const AVATAR_KEY: &str = "avatar";

/// The most nicks one `/api/profiles` request asks about.
pub const PROFILES_PER_ASK: usize = 200;

/// What's known of how someone shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Shown {
    /// Their name, when their profile gives one.
    name: Option<String>,
    /// Their picture's address, when they have one.
    avatar: Option<String>,
    /// Whether the server said their picture, or that they have none: until it has, a name
    /// it said doesn't make their picture known, and `/api/profiles` is asked for it.
    avatar_told: bool,
}

/// Folded nick → how they show, for everyone something is known of.
pub(super) type Names = HashMap<String, Shown>;

/// What kind of someone a [`Mention`] is, in the order the composer lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MentionKind {
    /// A person, or a guest.
    Person,
    /// An agent session.
    Agent,
    /// One of zirc's apps, CI or Alerts.
    App,
}

/// Someone the composer offers as a person types `@` and the start of a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    /// What the mention writes after the `@`, which wakes them: a person's or app's nick,
    /// `ada`; an agent session's id, `cc-598aa1`, which apps show as its name.
    pub token: String,
    /// Their nick: for an agent session, its machine's, `mattias-mbp`.
    pub nick: String,
    /// The name they show as, as their rows head them: `Ada Lovelace`, `zirc-chat`, `CI`.
    pub name: String,
    /// What follows the name, muted: `mattias's agent` for an agent session.
    pub detail: Option<String>,
    /// What kind of someone they are.
    pub kind: MentionKind,
    /// Their avatar, as their rows draw it: see [`Chat::speaker`].
    pub avatar: super::Avatar,
}

impl Chat {
    /// Who the composer offers for `@` and `prefix` in the conversation shown: the people in
    /// it other than us, the agent sessions that have spoken in it, and zirc's apps in it,
    /// in that order. Each matches when its token, nick or shown name starts with `prefix`,
    /// or any word of its name does, ignoring case; within a kind, those whose token, nick
    /// or name starts with it come first, then by name. At most `limit`.
    #[must_use]
    pub fn mentions(&self, prefix: &str, limit: usize) -> Vec<Mention> {
        let wanted = fold(prefix);
        let conversation = self.selected();
        let mut offered: Vec<Mention> = Vec::new();
        let people = conversation
            .members
            .values()
            .map(|member| member.nick.as_str())
            .chain((conversation.kind == super::Kind::Query).then_some(conversation.name.as_str()))
            .filter(|nick| !nick.eq_ignore_ascii_case(&self.me));
        for nick in people {
            let (name, kind) = match self.sender_kind(nick) {
                super::SenderKind::Person => (self.shown_name(nick), MentionKind::Person),
                super::SenderKind::Guest => (self.guest_name(nick), MentionKind::Person),
                // Agents are offered by session, apps by their rows' names, below.
                super::SenderKind::Agent | super::SenderKind::Integration => continue,
            };
            offered.push(Mention {
                token: nick.to_owned(),
                nick: nick.to_owned(),
                name,
                detail: None,
                kind,
                avatar: super::Avatar::Person {
                    nick: nick.to_owned(),
                },
            });
        }
        // Each agent session by its latest post here, headed as its rows are.
        let mut sessions: Vec<&str> = Vec::new();
        for entry in conversation.entries.iter().rev() {
            let super::EntryKind::Zirc(message) = &entry.kind else {
                continue;
            };
            let session = message.session.as_str();
            if self.sender_kind(&entry.from) != super::SenderKind::Agent
                || session.is_empty()
                || session == zirc_proto::NO_SESSION
                || session.eq_ignore_ascii_case(&entry.from)
                || sessions.contains(&session)
            {
                continue;
            }
            sessions.push(session);
            let speaker = self.speaker(entry);
            offered.push(Mention {
                token: session.to_owned(),
                nick: entry.from.clone(),
                name: speaker.name,
                detail: speaker.detail,
                kind: MentionKind::Agent,
                avatar: speaker.avatar,
            });
        }
        offered.extend(
            self.app_rows(&conversation.name)
                .into_iter()
                .map(|app| Mention {
                    token: app.nick.clone(),
                    nick: app.nick,
                    name: app.label,
                    detail: None,
                    kind: MentionKind::App,
                    avatar: app.avatar,
                }),
        );
        let mut found: Vec<(MentionKind, bool, String, Mention)> = offered
            .into_iter()
            .filter_map(|mention| {
                let name = fold(&mention.name);
                let starts = fold(&mention.token).starts_with(&wanted)
                    || fold(&mention.nick).starts_with(&wanted)
                    || name.starts_with(&wanted);
                let word = name
                    .split_whitespace()
                    .any(|word| word.starts_with(&wanted));
                (starts || word).then_some((mention.kind, !starts, name, mention))
            })
            .collect();
        found.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
        found.dedup_by(|a, b| a.3.token.eq_ignore_ascii_case(&b.3.token));
        found.into_iter().take(limit).map(|(.., m)| m).collect()
    }

    /// The initials `nick`'s avatar shows in every app: those of the name they show as
    /// ([`Chat::shown_name`]), `MC` for Maya Chen, else of their nick.
    #[must_use]
    pub fn avatar_initials(&self, nick: &str) -> String {
        super::initials(&self.shown_name(nick))
    }

    /// The name `nick` shows as: the one their profile gives them in the organisation; else
    /// a person's real name, which their apps send from their profile, unless it's their nick
    /// or they're a machine (whose real name is its owner) or a guest (named as
    /// [`Chat::guest_name`] says); else their nick.
    #[must_use]
    pub fn shown_name(&self, nick: &str) -> String {
        if let Some(name) = self
            .names
            .get(&fold(nick))
            .and_then(|shown| shown.name.clone())
        {
            return name;
        }
        self.realname(nick)
            .map(str::trim)
            .filter(|real| !real.is_empty() && !real.eq_ignore_ascii_case(nick))
            .filter(|_| self.machine_owner(nick).is_none() && !self.is_guest(nick))
            .map_or_else(|| nick.to_owned(), str::to_owned)
    }

    /// The address of `nick`'s picture, or `None` for their initials.
    #[must_use]
    pub fn avatar_url(&self, nick: &str) -> Option<&str> {
        self.names
            .get(&fold(nick))
            .and_then(|shown| shown.avatar.as_deref())
    }

    /// Takes in a page of zirc.dev's `/api/profiles` from the web app at `web`, such as
    /// `https://zirc.dev`, where the pictures are. What the server says live wins: a page
    /// only fills in what isn't known.
    pub fn take_profiles(&mut self, profiles: &Profiles, web: &str) {
        for (nick, profile) in profiles {
            let shown = self.names.entry(fold(nick)).or_default();
            if shown.name.is_none() {
                shown.name = Some(profile.name.clone());
            }
            if shown.avatar.is_none() {
                shown.avatar = profile.avatar.as_ref().map(|version| {
                    format!(
                        "{}{}?v={version}",
                        web.trim_end_matches('/'),
                        zirc_proto::media::avatar_path(nick)
                    )
                });
            }
        }
    }

    /// Those in our conversations whose profiles aren't known yet, at most
    /// [`PROFILES_PER_ASK`], to ask zirc.dev's `/api/profiles` about: each only once, until
    /// [`Chat::ask_profiles_again`]. Someone whose name the server said but not their picture
    /// is asked about too, for their picture.
    pub fn profiles_to_ask(&mut self) -> Vec<String> {
        let mut nicks: Vec<String> = self
            .conversations
            .iter()
            .flat_map(|conversation| conversation.members.values())
            .map(|member| member.nick.clone())
            .filter(|nick| {
                self.names
                    .get(&fold(nick))
                    .is_none_or(|shown| shown.avatar.is_none() && !shown.avatar_told)
            })
            .filter(|nick| !self.asked_profiles.contains(&fold(nick)))
            .collect();
        nicks.sort();
        nicks.dedup_by(|a, b| fold(a) == fold(b));
        nicks.truncate(PROFILES_PER_ASK);
        self.asked_profiles
            .extend(nicks.iter().map(|nick| fold(nick)));
        nicks
    }

    /// A profile key the server says of `target`: its name or picture, `value` none when
    /// it's gone. Whether it was one of those.
    pub(super) fn profile_metadata(
        &mut self,
        target: &str,
        key: &str,
        value: Option<&str>,
    ) -> bool {
        if key != NAME_KEY && key != AVATAR_KEY {
            return false;
        }
        let value = value.filter(|value| !value.is_empty()).map(str::to_owned);
        let shown = self.names.entry(fold(target)).or_default();
        if key == NAME_KEY {
            shown.name = value;
        } else {
            shown.avatar = value;
            shown.avatar_told = true;
        }
        true
    }

    /// Forgets that `nicks` were asked about, so [`Chat::profiles_to_ask`] gives them again:
    /// for an answer that was never taken in, as when the organisation shown changed while
    /// it came.
    pub fn ask_profiles_again(&mut self, nicks: &[String]) {
        for nick in nicks {
            self.asked_profiles.remove(&fold(nick));
        }
    }
}

/// The `@` mention being typed in `text` at `cursor`, a byte offset: where its `@` is and
/// what's typed after it up to the cursor, `(4, "ad")` for `"hi, @ad"` at its end. `None`
/// when the cursor isn't in one: an `@` counts only at the start or after a space or
/// punctuation, so an email address isn't one.
#[must_use]
pub fn mention_query(text: &str, cursor: usize) -> Option<(usize, &str)> {
    let cursor = floor_char(text, cursor);
    let before = &text[..cursor];
    let start = before
        .trim_end_matches(crate::names::nick_char)
        .strip_suffix('@')?
        .len();
    let opens = text[..start]
        .chars()
        .next_back()
        .is_none_or(|c| !crate::names::nick_char(c));
    opens.then(|| (start, &before[start + 1..]))
}

/// `text` with the `@` mention being typed at `cursor` (see [`mention_query`]) completed
/// to `mention`: the whole word replaced by `@<token>` and a space, the cursor after the
/// space, so what's typed next follows it. A space already after the word is used, not
/// doubled. `text` as it is when the cursor isn't in a mention.
#[must_use]
pub fn complete_mention(text: &str, cursor: usize, mention: &Mention) -> (String, usize) {
    let cursor = floor_char(text, cursor);
    let Some((start, _)) = mention_query(text, cursor) else {
        return (text.to_owned(), cursor);
    };
    let end = text[cursor..]
        .char_indices()
        .find(|&(_, c)| !crate::names::nick_char(c))
        .map_or(text.len(), |(at, _)| cursor + at);
    let after = &text[end..];
    let space = if after.starts_with(' ') { "" } else { " " };
    let completed = format!("{}@{}{space}{after}", &text[..start], mention.token);
    let cursor = start + 1 + mention.token.len() + 1;
    (completed, cursor)
}

/// `at` moved back to the nearest character boundary of `text`, and within it.
fn floor_char(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}
