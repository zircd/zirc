//! The channels - listing, browsing, joining, leaving and who is in them.

use super::{
    Attendance, Chat, Entry, EntryKind, Kind, Line, WHO_TOKEN, agent, agents_room, fold, group,
    is_channel, is_group, joins, nick_of, some, strip_formatting, target,
};

impl Chat {
    /// The server's channels and how many are in each, as it last listed them. Every app
    /// lists from this: the server keeps no channel nobody wants, so none is hidden here.
    #[must_use]
    pub fn listed(&self) -> Vec<(String, usize)> {
        self.listed.clone()
    }

    /// The channels we aren't in, to pick from and join with [`Chat::join`]: those with
    /// people in them first, the most people first, then by name. Group conversations are
    /// not channels to browse. One we left that we have history of stays, even when
    /// the server no longer lists it because nobody is in it, so the person can join it
    /// again.
    #[must_use]
    pub fn browse(&self) -> Vec<(String, usize)> {
        let mut channels: Vec<(String, usize)> = self
            .listed()
            .into_iter()
            .filter(|(name, _)| {
                !is_group(name) && !self.conversation(name).is_some_and(|c| c.joined)
            })
            .collect();
        for conversation in &self.conversations {
            let name = &conversation.name;
            if conversation.kind == Kind::Channel
                && !conversation.joined
                && !is_group(name)
                && self.last_said(name).is_some()
                && !self
                    .listed
                    .iter()
                    .any(|(listed, _)| listed.eq_ignore_ascii_case(name))
            {
                channels.push((name.clone(), 0));
            }
        }
        channels.sort_by(|(a, x), (b, y)| y.cmp(x).then_with(|| fold(a).cmp(&fold(b))));
        channels
    }

    /// Of [`Chat::browse`], those `query` names, as every app's Browse channels filters
    /// them while the person types: a name or a room's own name that starts with it first,
    /// then the rest that hold it, each in [`Chat::browse`]'s order. A leading `#` and the
    /// case don't matter; an empty query is all of them.
    #[must_use]
    pub fn browse_matching(&self, query: &str) -> Vec<(String, usize)> {
        let query = fold(query.trim().trim_start_matches('#'));
        if query.is_empty() {
            return self.browse();
        }
        let starts = |name: &str| {
            let name = fold(name.trim_start_matches('#'));
            name.starts_with(&query)
                || name
                    .rsplit_once('/')
                    .is_some_and(|(_, room)| room.starts_with(&query))
        };
        let (mut first, rest): (Vec<_>, Vec<_>) = self
            .browse()
            .into_iter()
            .filter(|(name, _)| fold(name).contains(&query))
            .partition(|(name, _)| starts(name));
        first.extend(rest);
        first
    }

    /// What Jump to offers last for what's typed, `query`, when that names a channel we
    /// aren't in: `Join #name` for one the server lists, `Create #name` for one it doesn't,
    /// as joining makes it (the user's choice, 11 Nov 2026). None when it isn't a channel's
    /// name, is a group conversation's, or is one we're in.
    #[must_use]
    pub fn join_offer(&self, query: &str) -> Option<JoinOffer> {
        let typed = query.trim().trim_start_matches(['#', '&']);
        if typed.is_empty()
            || typed
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || c == ',')
        {
            return None;
        }
        let name = format!("#{typed}");
        let ours = self
            .conversation(&name)
            .is_some_and(|conversation| conversation.joined);
        if ours || is_group(&name) {
            return None;
        }
        let listed = self
            .listed
            .iter()
            .any(|(listed, _)| listed.eq_ignore_ascii_case(&name));
        Some(JoinOffer {
            label: if listed {
                join_label(&name)
            } else {
                create_label(&name)
            },
            create: !listed,
            name,
        })
    }

    /// Shows the channel or private conversation called `name`, making it if there isn't
    /// one: with an agent session, `<machine>/<session>`, too. A channel we're not in is joined; a private conversation's history is asked
    /// for. Returns what to send.
    pub fn open(&mut self, name: &str) -> Vec<Line> {
        if self.is_server(name) {
            self.show_server();
            return Vec::new();
        }
        if is_channel(name) {
            return self.join(&[name.to_owned()]);
        }
        let fresh = self.position(name).is_none();
        let at = match agent(name) {
            Some((nick, session)) => self.ensure_agent(nick, session),
            None => self.ensure(name, Kind::Query),
        };
        let mut lines = self.select(at);
        if fresh {
            if self.link.as_ref().is_some_and(|link| link.chathistory) {
                lines.push(self.latest(target(name)));
            }
            lines.extend(self.ask_read(name));
        }
        lines
    }

    /// Joins `channels`, showing the first once joined, or now if it is already.
    pub fn join(&mut self, channels: &[String]) -> Vec<Line> {
        let Some(first) = channels.first() else {
            return Vec::new();
        };
        let mut lines = match self.position(first) {
            Some(at) if self.conversations[at].joined => self.select(at),
            _ => {
                self.show_on_join = Some(first.clone());
                Vec::new()
            }
        };
        let wanted: Vec<String> = channels
            .iter()
            .filter(|channel| !self.conversation(channel).is_some_and(|c| c.joined))
            .cloned()
            .collect();
        if !self.listed_once {
            self.asked
                .extend(wanted.iter().map(|channel| fold(channel)));
        }
        lines.extend(joins(&wanted));
        lines
    }

    /// Leaves `channel`. The server keeps an always-on account's channels, so what is left
    /// stays left across restarts and devices with nothing stored here.
    pub fn part(&mut self, channel: &str, reason: Option<&str>) -> Vec<Line> {
        self.joins
            .retain(|join| !join.eq_ignore_ascii_case(channel));
        // A guest's room left is hidden too: out of the list till something new is said in
        // it, as one we never joined is when hidden ([`Chat::close_direct`]).
        if zirc_proto::guest_room_of(channel).is_some() {
            self.closed.insert(fold(channel), (self.clock)());
        }
        if let Some(at) = self.position(channel) {
            let standing = &mut self.conversations[at].standing;
            if *standing == Attendance::Rejoin {
                *standing = Attendance::Fresh;
            }
        }
        vec![match reason {
            Some(reason) => Line::new("PART", [channel, reason]),
            None => Line::new("PART", [channel]),
        }]
    }

    /// What to ask before leaving `channel`, which every app asks in these words. Where we
    /// are the last one in it, the server won't list it once we've gone (after a restart, we
    /// have no history of it either), so Browse can't offer it again, and the prompt says so.
    #[must_use]
    pub fn leave_prompt(&self, channel: &str) -> String {
        // zirc's apps, an integration among them, are no one to leave it to.
        let last = self.conversation(channel).is_some_and(|c| {
            c.joined && c.members.values().filter(|m| !self.is_app(&m.nick)).count() <= 1
        });
        if last {
            format!(
                "Leave {channel}? You're the last one here, so it will be gone from the list; ask someone to invite you or join it by name with /join."
            )
        } else {
            format!("Leave {channel}? You can join it again from Browse channels.")
        }
    }

    /// Closes the conversation at `at`: a channel we're in is left, and the rest are
    /// forgotten. The server's can't be closed.
    pub fn close(&mut self, at: usize) -> Vec<Line> {
        let Some(conversation) = self.conversations.get(at) else {
            return Vec::new();
        };
        match conversation.kind {
            Kind::Server => Vec::new(),
            Kind::Channel if conversation.joined => {
                let name = conversation.name.clone();
                self.part(&name, None)
            }
            Kind::Channel | Kind::Query => {
                self.conversations.remove(at);
                self.reindex();
                if self.selected > at || self.selected >= self.conversations.len() {
                    self.selected -= 1;
                }
                self.select(self.selected)
            }
        }
    }

    /// A channel we left is no longer the one shown: the one before it in the list is, or
    /// the server's.
    pub(super) fn stop_showing(&mut self, at: usize) -> Vec<Line> {
        if self.selected != at {
            return Vec::new();
        }
        let to = (0..at)
            .rev()
            .find(|&before| self.conversations[before].standing != Attendance::Left)
            .unwrap_or(0);
        self.select(to)
    }

    /// People coming, going, and changing their names and topics.
    pub(super) fn membership(&mut self, line: &Line, history: bool) -> Vec<Line> {
        // History replays joins, parts, nick changes and topics as they were (event-playback);
        // who's here and the topic now come from joining, so the past changes none of it.
        if history {
            return Vec::new();
        }
        let param = |i: usize| line.params.get(i).map_or("", String::as_str);
        let from = nick_of(line);
        let me = from.eq_ignore_ascii_case(&self.me);
        let time = self.time_of(line);
        let event = |kind| Entry {
            time,
            id: line.tag("msgid").map(str::to_owned),
            from: from.to_owned(),
            owner: super::owner_by_host(line).map(str::to_owned),
            kind,
            history: false,
            mention: false,
            sent_from: None,
            reply_to: None,
            formatted: None,
            edited: false,
            sealed: None,
        };
        let mut lines = Vec::new();
        match line.command.as_str() {
            "JOIN" => return self.joined(line),
            "PART" => {
                let channel = param(0);
                self.presence.left(channel, from);
                if let Some(at) = self.position(channel) {
                    let conversation = &mut self.conversations[at];
                    let was_in = if me {
                        conversation.joined = false;
                        conversation.standing = Attendance::Left;
                        conversation.members.clear();
                        true
                    } else {
                        conversation.remove_member(from)
                    };
                    if was_in {
                        self.push(at, event(EntryKind::Part(some(param(1)))));
                    }
                    if me {
                        lines.extend(self.stop_showing(at));
                    }
                }
            }
            "KICK" => {
                let (channel, nick) = (param(0), param(1));
                self.presence.left(channel, nick);
                if let Some(at) = self.position(channel) {
                    let conversation = &mut self.conversations[at];
                    if nick.eq_ignore_ascii_case(&self.me) {
                        conversation.joined = false;
                        conversation.standing = Attendance::Left;
                        conversation.members.clear();
                    } else {
                        conversation.remove_member(nick);
                    }
                    let kind = EntryKind::Kick {
                        nick: nick.to_owned(),
                        reason: some(param(2)),
                    };
                    self.push(at, event(kind));
                    if nick.eq_ignore_ascii_case(&self.me) {
                        lines.extend(self.stop_showing(at));
                    }
                }
            }
            "QUIT" => {
                self.presence.quit(from);
                self.live_gone(from, time);
                self.away.remove(&fold(from));
                for at in 0..self.conversations.len() {
                    let conversation = &mut self.conversations[at];
                    if conversation.remove_member(from)
                        || (conversation.kind == Kind::Query && conversation.is(from))
                    {
                        self.push(at, event(EntryKind::Quit(some(param(0)))));
                    }
                }
            }
            "NICK" => self.renamed(from, param(0), time),
            "TOPIC" => {
                if let Some(at) = self.position(param(0)) {
                    let text = strip_formatting(param(1));
                    self.conversations[at].topic = some(&text);
                    self.push(at, event(EntryKind::Topic(text)));
                }
            }
            _ => {}
        }
        lines
    }

    /// Someone joined a channel: us, which shows it and asks for its history, or someone
    /// else.
    pub(super) fn joined(&mut self, line: &Line) -> Vec<Line> {
        let channel = line.params.first().map_or("", String::as_str);
        let from = nick_of(line);
        let me = from.eq_ignore_ascii_case(&self.me);
        // extended-join: the account, `*` for none, then the real name.
        if let Some(realname) = line.params.get(2) {
            self.realnames.insert(fold(from), realname.clone());
            let account = &line.params[1];
            self.accounts
                .insert(fold(from), (account != "*").then(|| account.clone()));
        }
        if !me {
            if let Some(at) = self.position(channel) {
                self.conversations[at].add_member(from);
                let entry = Entry {
                    time: self.time_of(line),
                    id: line.tag("msgid").map(str::to_owned),
                    from: from.to_owned(),
                    owner: super::owner_by_host(line).map(str::to_owned),
                    kind: EntryKind::Join,
                    history: false,
                    mention: false,
                    sent_from: None,
                    reply_to: None,
                    formatted: None,
                    edited: false,
                    sealed: None,
                };
                self.push(at, entry);
            }
            return Vec::new();
        }
        // Put in a channel before we asked for any: the server keeps ours.
        if self.options.list_channels && !self.listed_once && !self.asked.contains(&fold(channel)) {
            self.keeps_channels = true;
        }
        let at = self.ensure(channel, Kind::Channel);
        let conversation = &mut self.conversations[at];
        let again = std::mem::replace(&mut conversation.joined, true);
        conversation.standing = Attendance::Fresh;
        conversation.members.clear();
        conversation.add_member(from);
        // Show a channel asked for, or the first one rather than the server's. A channel
        // joined again, as the server can say when the person's other apps join it too,
        // leaves the conversation shown as it was.
        let asked = self
            .show_on_join
            .as_deref()
            .is_some_and(|c| c.eq_ignore_ascii_case(channel));
        let earlier = match self.selected().kind {
            Kind::Server => true,
            _ => self.first_shown && at < self.selected,
        };
        // A group is one of the person's conversations, not where an organisation talks: it
        // never takes the first channel's place.
        let first = self.options.show_first_channel && !again && earlier && !is_group(channel);
        let mut lines = Vec::new();
        if asked || first {
            self.show_on_join = None;
            lines = self.select(at);
            self.first_shown = !asked;
        }
        if let Some(members) = self.making.remove(&fold(channel)) {
            lines.extend(group::set_up(channel, &members));
        }
        // The real names of those already here, which extended-join won't say.
        lines.push(Line::new("WHO", [channel, &format!("%tnfhar,{WHO_TOKEN}")]));
        // In the organisation's channel, a device without its key asks for it.
        if channel.eq_ignore_ascii_case(super::ORG_CHANNEL) {
            lines.extend(self.ask_for_key(None));
        }
        if self.link.as_ref().is_some_and(|l| l.chathistory) {
            lines.push(self.catch_up(channel));
        }
        // A repository's channel comes with its agents' room, where its agents talk, when the
        // server lists one: never one made by asking.
        let room = agents_room(channel);
        let listed = self
            .listed
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case(&room));
        // One we have been in and left is still here, not joined again by itself.
        if !again && listed && self.conversation(&room).is_none() {
            lines.extend(joins(&[room]));
        }
        lines
    }

    /// We're invited to a channel. A group's invitation, or one of our own machines', as
    /// zircd invites its owner to a repository's rooms when it starts watching it, is joined
    /// without showing it: one that arrives live, or one from the private history with
    /// whoever invited us that's newer than how far we've read it, which an app that's been
    /// away hasn't seen. One we have read was joined then, or left since. Other invitations
    /// are for the person to take up.
    pub(super) fn invited(&mut self, line: &Line, history: bool) -> Vec<Line> {
        let (Some(nick), Some(channel)) = (line.params.first(), line.params.get(1)) else {
            return Vec::new();
        };
        let from = nick_of(line);
        if !nick.eq_ignore_ascii_case(&self.me) {
            return Vec::new();
        }
        // A group's invitation, or one of our own machines' to a repository it started
        // watching (its channel and agents' room), is taken up as it comes; anyone else's
        // is the person's to take up.
        if !is_group(channel) && !self.ours(from) {
            if !history {
                self.note(format!("{from} invites you to {channel}"));
            }
            return Vec::new();
        }
        let time = self.time_of(line);
        let seen = history
            && self
                .conversation(from)
                .and_then(|c| c.read)
                .is_some_and(|read| read >= time);
        let joined = self.conversation(channel).is_some_and(|c| c.joined);
        // One we were in and left is still here, and its old invitation isn't taken up again.
        if seen || joined || (history && self.position(channel).is_some()) {
            return Vec::new();
        }
        joins(std::slice::from_ref(channel))
    }
}

/// A channel we aren't in, to browse and join, as every app lists it ([`Chat::browse`]): the
/// phones read this record through `UniFFI`, the web chat as JSON.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
pub struct BrowseRow {
    /// The channel: `#web`.
    pub name: String,
    /// How many are in it, as every app says it ([`super::members_label`]): `3 members`. None when
    /// nobody is in it now, as for one we left that has history.
    pub members: Option<String>,
}

/// `channels` with how many are in each, as [`Chat::browse`] lists them, as rows.
#[must_use]
pub fn browse_rows(channels: Vec<(String, usize)>) -> Vec<BrowseRow> {
    channels
        .into_iter()
        .map(|(name, count)| BrowseRow {
            name,
            members: (count > 0).then(|| super::members_label(count)),
        })
        .collect()
}

/// Jump to's row for a channel nothing it lists is ([`Chat::join_offer`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
pub struct JoinOffer {
    /// The channel, `#name`, as [`Chat::open`] joins it.
    pub name: String,
    /// What the row says: `Join #name` or `Create #name`.
    pub label: String,
    /// Whether the server doesn't list it, so joining makes it.
    pub create: bool,
}

/// Jump to's row for a channel the server lists that we aren't in: `Join #name`.
#[must_use]
pub fn join_label(name: &str) -> String {
    format!("Join {name}")
}

/// Jump to's row for a channel the server doesn't list, which joining makes: `Create #name`.
#[must_use]
pub fn create_label(name: &str) -> String {
    format!("Create {name}")
}
