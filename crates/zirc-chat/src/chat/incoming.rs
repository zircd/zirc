//! What arrives - replies, messages and tags - and how it is taken in.

use super::{
    Attendance, CLIENT_TAG, Chat, DEFAULT_CHANNELS, Entry, EntryKind, Event, Kind, Line, Logged,
    REACT, REPLY, Reading, SESSION_MARK, UNREACT, WHO_TOKEN, agent, by_choice, entry_of, fold,
    for_agent, formatted_text, is_channel, is_service, joins, named_session, nick_of, some,
    strip_formatting,
};
use chrono::{DateTime, Utc};
use zirc_proto::{Level, Message};

impl Chat {
    /// The server has listed its channels (`RPL_LISTEND`): joins what was asked for and what
    /// was open before a reconnect and not left since (for a server that doesn't keep our
    /// channels). Channels are joined by choice, never because they are listed: an always-on
    /// account keeps its own, so what was left stays left, across restarts and devices, with
    /// nothing stored here. Such a server has put us back in ours by now, before it answered
    /// `LIST`: one we were in that it left out was left on another device while this one was
    /// away, and joining it again would undo that for them all. Only a person in no channel
    /// at all, who asked for none, is put in the [`DEFAULT_CHANNELS`] that exist, once. A
    /// channel the server no longer lists is gone, a branch's removed after it was merged:
    /// joining it would make it again.
    fn list_ended(&mut self) -> Vec<Line> {
        self.listed = std::mem::take(&mut self.listing);
        let mut lines = Vec::new();
        if self.keeps_channels {
            for at in 0..self.conversations.len() {
                let conversation = &mut self.conversations[at];
                if conversation.kind == Kind::Channel && conversation.standing == Attendance::Rejoin
                {
                    conversation.standing = Attendance::Left;
                    lines.extend(self.stop_showing(at));
                }
            }
        }
        let asked: &[String] = if self.keeps_channels && self.joins_sent {
            &[]
        } else {
            &self.joins
        };
        let mut wanted: Vec<String> = Vec::new();
        let listed = |name: &str| {
            self.listed
                .iter()
                .any(|(listed, _)| listed.eq_ignore_ascii_case(name))
        };
        let open = self
            .conversations
            .iter()
            .filter(|c| c.kind == Kind::Channel && c.standing == Attendance::Rejoin)
            .filter(|c| !by_choice(&c.name) || listed(&c.name))
            .map(|c| &c.name);
        for channel in asked.iter().chain(open) {
            let joined = self.conversation(channel).is_some_and(|c| c.joined);
            if !joined && !wanted.iter().any(|w| w.eq_ignore_ascii_case(channel)) {
                wanted.push(channel.clone());
            }
        }
        let in_none = !self
            .conversations
            .iter()
            .any(|c| c.kind == Kind::Channel && (c.joined || c.standing == Attendance::Rejoin));
        let first = !std::mem::replace(&mut self.listed_once, true);
        self.joins_sent = true;
        // On a server that keeps our channels, being in none is having left them.
        if first && wanted.is_empty() && in_none && !self.keeps_channels {
            wanted = DEFAULT_CHANNELS
                .iter()
                .filter(|name| listed(name))
                .map(|name| (*name).to_owned())
                .collect();
        }
        lines.extend(joins(&wanted));
        lines
    }

    /// Numeric replies and errors.
    pub(super) fn reply(&mut self, line: &Line) -> Vec<Line> {
        let param = |i: usize| line.params.get(i).map_or("", String::as_str);
        match line.command.as_str() {
            // ERR_NOSUCHNICK for the tenant's pusher, told what was read (Chat::mark): a
            // server without one isn't the person's to hear about.
            "401" if param(1).eq_ignore_ascii_case(zirc_proto::push::PUSHER) => {}
            // RPL_TOPIC
            "332" => {
                if let Some(at) = self.position(param(1)) {
                    self.conversations[at].topic = some(&strip_formatting(param(2)));
                }
            }
            // RPL_NAMREPLY
            "353" => {
                if let Some(at) = self.position(param(2)) {
                    let conversation = &mut self.conversations[at];
                    if conversation.names_done {
                        conversation.members.clear();
                        conversation.names_done = false;
                    }
                    for nick in param(3).split(' ') {
                        conversation.add_member(nick);
                    }
                }
            }
            // RPL_ENDOFNAMES
            "366" => return self.names_done(param(1)),
            // WHOX's answer to our WHO: me, token, host, nick, flags (`H` here or `G` gone,
            // then more), account (`0` for none), real name.
            "354" if param(1) == WHO_TOKEN => {
                self.hosted(param(3), param(2));
                self.whox(param(3), param(4), param(5), param(6));
            }
            // A plain WHO answer, from servers without WHOX: the host, the flags, then the
            // real name after the hop count in the last parameter.
            "352" => {
                let realname = param(7).split_once(' ').map_or("", |(_, name)| name);
                self.realnames.insert(fold(param(5)), realname.to_owned());
                self.hosted(param(5), param(3));
                self.flagged(param(5), param(6));
            }
            // WHOIS answers, for profiles.
            "311" | "330" | "317" => self.whois_reply(line),
            // RPL_AWAY, as a message to someone away is answered.
            "301" => {
                self.away.insert(fold(param(1)), param(2).to_owned());
            }
            // RPL_UNAWAY and RPL_NOWAWAY: we're back, or away.
            "305" => self.away_reply(false),
            "306" => self.away_reply(true),
            // RPL_LIST
            "322" => {
                let users = param(2).parse().unwrap_or(0);
                self.listing.push((param(1).to_owned(), users));
            }
            // RPL_LISTEND
            "323" => return self.list_ended(),
            // The MOTD is the Server channel's ([`Chat::server_window`]); its line that says
            // the server logs what people send is seen where they type, too.
            "372" => {
                if let Some(logged) = Logged::from_motd(param(1)) {
                    self.status.logged = Some(logged);
                }
            }
            // The server closing the connection; the rest of registration's chatter, the
            // welcome included, is left out.
            "ERROR" => {
                let text = strip_formatting(&line.params.join(" "));
                self.server_notice(self.time_of(line), "", text);
            }
            // Errors answer what the person did, so they show where the person is looking.
            // Subscribing to profiles is the app's doing, not the person's: a server that
            // won't has nothing to tell them.
            "FAIL" if param(0) == "METADATA" => {}
            // A link's message the server doesn't have: it shows at the end, unremarked.
            "FAIL" if param(0) == "CHATHISTORY" && self.history_failed(line) => {}
            "FAIL" if param(0) == "REDACT" => {
                let last = line.params.last().map_or("", String::as_str);
                self.note(format!("couldn't trim the message: {last}"));
            }
            "FAIL" => {
                let last = line.params.last().map_or("", String::as_str);
                self.error(line, format!("{}: {last}", param(0)));
            }
            // ERR_NOMOTD: the Server channel says so ([`Chat::server_window`]).
            "422" => {}
            numeric if numeric.starts_with(['4', '5']) && numeric.len() == 3 => {
                self.error(line, line.params.get(1..).unwrap_or_default().join(" "));
            }
            _ => {}
        }
        Vec::new()
    }

    /// Reactions and client tags on a TAGMSG naming the message they answer.
    pub(super) fn tagmsg(&mut self, line: &Line) -> Vec<Line> {
        // A working set, kept out of history: read as the PRIVMSG it stands for.
        if let Some(privmsg) = crate::presence::as_privmsg(line) {
            return self.message(&privmsg, false);
        }
        if crate::presence::asks(line) {
            return Vec::new();
        }
        if self.typing_tag(line) {
            return Vec::new();
        }
        if let Some(lines) = self.sealing_tag(line) {
            return lines;
        }
        let from = nick_of(line);
        let target = line.params.first().map_or("", String::as_str);
        if self.receipt_tag(line, from) {
            return Vec::new();
        }
        let (Some(id), Some((emoji, on))) = (
            line.tag(REPLY),
            line.tag(REACT)
                .or_else(|| line.tag("+react"))
                .map(|e| (e, true))
                .or_else(|| line.tag(UNREACT).map(|e| (e, false))),
        ) else {
            return Vec::new();
        };
        let name = if is_channel(target) || from.eq_ignore_ascii_case(&self.me) {
            target
        } else {
            from
        };
        // A reaction in a conversation with an agent goes to its machine.
        let at = self
            .position(name)
            .filter(|&at| self.conversations[at].holds(id))
            .or_else(|| self.conversations.iter().position(|c| c.holds(id)))
            .or_else(|| self.position(name));
        if let Some(at) = at {
            let time = self.time_of(line);
            self.conversations[at].react(id, from, emoji, on, time);
        }
        Vec::new()
    }

    /// Takes `line`, a `TAGMSG` from `from`, as a machine's word on a message we sent one
    /// of its agent sessions, if it is one ([`zirc_proto::receipt`]): kept for that session's
    /// conversation, in place of any earlier one. Only the machine's own word counts, as the
    /// conversation is keyed by who sent it; our own machines' receipts to others aren't ours.
    fn receipt_tag(&mut self, line: &Line, from: &str) -> bool {
        let Some(value) = line.tag(zirc_proto::receipt::TAG) else {
            return false;
        };
        if let (Some(id), Some(receipt)) =
            (line.tag(REPLY), zirc_proto::receipt::Receipt::parse(value))
            && !from.eq_ignore_ascii_case(&self.me)
        {
            let name = super::fold(&super::agent_channel(from, &receipt.session));
            if let Some(at) = self.position(&name) {
                self.conversations[at].touch(id);
            }
            self.receipts.insert(name, (id.to_owned(), receipt));
        }
        true
    }

    /// What the agent conversation `name`'s machine last said of a message we sent its
    /// session, and that message's id.
    pub(super) fn receipt(&self, name: &str) -> Option<&(String, zirc_proto::receipt::Receipt)> {
        self.receipts.get(&super::fold(name))
    }

    pub(super) fn message(&mut self, line: &Line, history: bool) -> Vec<Line> {
        // A working set comes only as a TAGMSG now: one in a plain PRIVMSG, as history holds
        // from before, says nothing about what anyone is changing.
        if crate::presence::old_working(line) {
            return Vec::new();
        }
        let param = |i: usize| line.params.get(i).map_or("", String::as_str);
        let notice = line.command == "NOTICE";
        let source = line.source.as_deref().unwrap_or_default();
        let from = nick_of(line);
        let target = param(0);
        let me = from.eq_ignore_ascii_case(&self.me);
        let from_server = !source.contains('!');
        // Ergo replays joins, quits and topic changes into history as HistServ's words, for
        // clients without event-playback: not worth showing.
        if history && from.eq_ignore_ascii_case("HistServ") {
            return Vec::new();
        }
        let services = !is_channel(target) && (is_service(from) || is_service(target));
        if services && me {
            return Vec::new();
        }
        // What the server and its services say is for the status, not a conversation.
        if (notice && (from_server || target == "*")) || services {
            let from = if from_server { "" } else { from };
            let text = strip_formatting(param(1));
            self.server_notice(self.time_of(line), from, text);
            return Vec::new();
        }
        let (name, kind) = if is_channel(target) {
            (target.to_owned(), Kind::Channel)
        } else if me {
            (target.to_owned(), Kind::Query)
        } else {
            (from.to_owned(), Kind::Query)
        };
        let Some((entry, words, files, body)) = said(line, param(1), notice) else {
            return Vec::new();
        };
        let entry = Entry {
            time: self.time_of(line),
            id: line.tag("msgid").map(str::to_owned),
            from: from.to_owned(),
            owner: super::owner_by_host(line).map(str::to_owned),
            mention: !me
                && kind != Kind::Server
                && (kind == Kind::Query || super::push::names_me(&entry, &words, &self.me)),
            sent_from: line
                .tag(CLIENT_TAG)
                .filter(|app| !app.is_empty())
                .map(str::to_owned),
            reply_to: line
                .tag(REPLY)
                .filter(|id| !id.is_empty())
                .map(str::to_owned),
            formatted: formatted_text(&entry, body),
            edited: false,
            sealed: super::SealedTag::of(line),
            files,
            kind: entry,
            history,
        };
        if self.corrected(line, &name, &entry) {
            return Vec::new();
        }
        // A machine saying which of its sessions are live may name one nothing loaded is with.
        let mut asked: Vec<Line> = Vec::new();
        if let EntryKind::Zirc(message) = &entry.kind {
            asked.extend(self.learn_said(&name, &entry.from, entry.time, message));
            // Intents and claims count from when they were said. A working set, never in
            // history, is never replaced by an older one, and is never an entry: who is
            // changing what is known from [`Chat::presence`].
            let working = matches!(message.event, Event::Working { .. });
            let current = !working || {
                let key = (fold(&name), fold(&entry.from));
                let newer = self.working_at.get(&key).is_none_or(|at| *at <= entry.time);
                if newer {
                    self.working_at.insert(key, entry.time);
                }
                newer
            };
            if current {
                let at = u64::try_from(entry.time.timestamp()).unwrap_or_default();
                self.presence.apply(&name, &entry.from, message, at, None);
                if working {
                    self.say_both_changing(&name, entry.time);
                }
            }
            // Nor is anything else only true now, such as what a session wants of a lease,
            // which zircd says again live when an app joins: never history, and no row
            // unless the person asked to see everything in this conversation.
            // A card's live update is the exception: it folds into its card.
            if message.is_live()
                && entry.live_card().is_none()
                && self
                    .conversation(&name)
                    .is_none_or(|conversation| conversation.threshold > Level::Debug)
            {
                return asked;
            }
        }
        let (entry, session) = self.for_agent(&name, entry);
        if let Some(session) = session {
            let at = self.ensure_agent(&name, &session);
            self.add(at, entry);
            return asked;
        }
        let fresh = self.position(&name).is_none();
        let at = self.ensure(&name, kind);
        self.add(at, entry);
        if fresh && kind == Kind::Query {
            asked.extend(self.ask_read(&name));
        }
        asked
    }

    /// What a zirc `message` from `from` in `name`, said at `time`, tells of its sender: its
    /// session's name, title and what it's doing, and its repository's forge.
    /// It gives what to ask the server for: older history, when a machine says a session is live that
    /// nothing loaded is with ([`Chat::seek_agents`]).
    fn learn_said(
        &mut self,
        name: &str,
        from: &str,
        time: DateTime<Utc>,
        message: &Message,
    ) -> Option<Line> {
        // A session's name, picture and title are its own machine's to say: the first nick
        // heard to speak as a session keeps it, so another member can't rename or retitle
        // someone's agent by sending its id.
        let heard = self.session_from.contains_key(&message.session);
        let theirs = self
            .session_from
            .entry(message.session.clone())
            .or_insert_with(|| from.to_ascii_lowercase())
            .eq_ignore_ascii_case(from);
        if theirs {
            self.learn_session(message);
        }
        // Heard for the first time, anywhere: what we wrote to it before is its.
        if !heard && theirs {
            self.adopt_agent(from, &message.session);
        }
        // A repository's forge, from a line in a room every repository shares, is its own.
        let place = crate::names::scope_of(name, message.repo.as_deref());
        self.learn_forge(&place, from, &message.event);
        if let Some(card) = zirc_proto::session::SessionCard::of(&message.event)
            && theirs
            && card.session == message.session
        {
            self.learn_title(card, time);
        }
        if let Some(live) = zirc_proto::session::Live::of(&message.event) {
            self.learn_live(from, live, time);
            return self.seek_agents(from);
        }
        None
    }

    /// `entry`, in the private conversation with `nick`, and the agent session it's with, if
    /// it's with one: a message a session on any machine the server vouches for sent with
    /// `zirc dm`, which carries its name, a teammate's agent's as our own's; or what we said
    /// to one of our own there, `<session> …` (older apps: `/agent <session> …`), once
    /// it has a conversation of its own (until then it stays with the machine's, to move
    /// when the session answers). What we said is shown without its `<session>`.
    pub(super) fn for_agent(&self, nick: &str, mut entry: Entry) -> (Entry, Option<String>) {
        if is_channel(nick) || self.whose(nick).is_none() {
            return (entry, None);
        }
        let session = if entry.from.eq_ignore_ascii_case(&self.me) {
            if !self.ours(nick) {
                return (entry, None);
            }
            let EntryKind::Message(text) = &entry.kind else {
                return (entry, None);
            };
            let said = self
                .conversations
                .iter()
                .find_map(|c| {
                    let (machine, session) = agent(&c.name)?;
                    let rest = for_agent(text, session)?;
                    machine
                        .eq_ignore_ascii_case(nick)
                        .then(|| (session.to_owned(), rest.to_owned()))
                })
                // Or a session the machine runs that has no conversation yet, as after the
                // app started again: what we said to it is its.
                .or_else(|| {
                    let (session, rest) = named_session(text)?;
                    self.known_on(nick, session)
                        .then(|| (session.to_owned(), rest.to_owned()))
                });
            let Some((session, rest)) = said else {
                return (entry, None);
            };
            entry.kind = EntryKind::Message(rest);
            session
        } else {
            match &entry.kind {
                EntryKind::Zirc(message)
                    if !message.session.is_empty()
                        && message.session != zirc_proto::NO_SESSION
                        && !message.session.contains(SESSION_MARK) =>
                {
                    message.session.clone()
                }
                _ => return (entry, None),
            }
        };
        (entry, Some(session))
    }

    /// An error the server answered with: where the person is looking, and in the Server
    /// channel, as an IRC client's server window keeps them, once.
    fn error(&mut self, line: &Line, text: String) {
        if self.selected().kind != Kind::Server {
            self.server_said(self.time_of(line), "", strip_formatting(&text));
        }
        self.note(text);
    }

    /// What IRC clients show in their server window, kept in the Server channel (the user's
    /// choice, 16 Nov 2026): the welcome, the server's features, how many are on, the MOTD,
    /// login results and our modes; and in the seconds after a line was typed there, every
    /// reply. Never the replies to what the apps ask by themselves (WHO, LIST, history), which
    /// come by the hundred. Notices, services and errors come by [`Chat::server_notice`] and
    /// [`Chat::error`].
    pub(super) fn server_window(&mut self, line: &Line) {
        let command = line.command.as_str();
        let numeric = command.len() == 3 && command.bytes().all(|b| b.is_ascii_digit());
        let typed = numeric
            && !command.starts_with(['4', '5'])
            && self.raw_until.is_some_and(|until| (self.clock)() <= until);
        let ours = command == "MODE"
            && line
                .params
                .first()
                .is_some_and(|target| target.eq_ignore_ascii_case(&self.me));
        let shown = matches!(
            command,
            "001"
                | "002"
                | "003"
                | "004"
                | "005"
                | "221"
                | "251"
                | "252"
                | "253"
                | "254"
                | "255"
                | "265"
                | "266"
                | "372"
                | "375"
                | "376"
                | "396"
                | "422"
                | "900"
                | "901"
                | "903"
                | "904"
                | "905"
                | "906"
                | "907"
                | "908"
        );
        if !(shown || typed || ours) {
            return;
        }
        // Our nick first, as every reply has it, is no one's news.
        let params = line.params.get(1..).unwrap_or_default();
        let text = match command {
            // Their words say the rest: `<nick!user@host> <account> :You are now logged in as
            // <account>`, `<now> <max> :Current local users 15, max 18`.
            "900" | "901" | "265" | "266" => params.last().cloned().unwrap_or_default(),
            "MODE" | "221" => format!("your modes: {}", params.join(" ")),
            _ => params.join(" "),
        };
        self.server_said(self.time_of(line), "", strip_formatting(&text));
    }

    /// What the server or one of its services (`from`, empty for the server) said: the status's
    /// latest, and the Server channel's ([`Chat::server_said`]).
    pub(super) fn server_notice(&mut self, time: DateTime<Utc>, from: &str, text: String) {
        self.server_said(time, from, text.clone());
        self.status.notice(time, from, text);
    }

    /// Keeps what `from` said in the server's own conversation, the Server channel: by the
    /// server's name when `from` is empty, and never unread (the user's choice, 16 Nov 2026),
    /// so it goes in as it is, not through [`Chat::add`].
    pub(super) fn server_said(&mut self, time: DateTime<Utc>, from: &str, text: String) {
        if text.is_empty() {
            return;
        }
        let Some(at) = self.position(&self.server.clone()) else {
            return;
        };
        // In the order it came, as a server's window shows it: what's typed there is stamped by
        // this computer's clock, which may be ahead of the server's answer to it.
        let time = self.conversations[at]
            .entries
            .last()
            .map_or(time, |last| time.max(last.time));
        let entry = Entry {
            time,
            id: None,
            from: if from.is_empty() {
                self.server.clone()
            } else {
                from.to_owned()
            },
            owner: None,
            kind: if from.eq_ignore_ascii_case(&self.me) {
                EntryKind::Message(text)
            } else {
                EntryKind::Notice(text)
            },
            history: false,
            mention: false,
            sent_from: None,
            reply_to: None,
            formatted: None,
            edited: false,
            sealed: None,
            files: Vec::new(),
        };
        let max = self.options.max_entries;
        let _ = self.conversations[at].insert(entry, max);
    }

    /// Adds a message to the conversation at `at`. It's fresh when it arrived live from
    /// someone else, and unread when it's from someone else and later than the server's
    /// read marker, or, with none, when it's fresh.
    pub(super) fn add(&mut self, at: usize, entry: Entry) {
        if self.is_blocked(&entry.from) {
            self.conversations[at].hide(entry);
            return;
        }
        let name = self.conversations[at].name.clone();
        self.arrived(&name, &entry.from);
        let theirs = !entry.from.eq_ignore_ascii_case(&self.me);
        let fresh = theirs && !entry.history && self.conversations[at].shows(&entry);
        // Unread is what [`Conversation::counts`] says, as every recount has it: a session's
        // title card is shown nowhere, so it's nothing to read.
        let unread = self.conversations[at].counts(&entry, &self.me)
            && match self.conversations[at].read {
                Some(read) => entry.time > read,
                None => fresh,
            };
        let mention = entry.mention;
        // What comes while the person reads further up, or isn't looking, starts the New
        // divider, as when they'd opened the conversation with it unread.
        let new = (fresh
            && at == self.selected
            && (self.reading == Reading::Up || !self.reads())
            && self.divider.is_none())
        .then(|| entry.id.clone())
        .flatten();
        let copy = fresh.then(|| entry.clone());
        // Only a card's latest live update counts: it says all its steps.
        if let Some(card) = entry.live_card() {
            self.conversations[at].entries.retain(|earlier| {
                earlier.live_card() != Some(card) || !earlier.from.eq_ignore_ascii_case(&entry.from)
            });
        }
        if !self.conversations[at].insert(entry, self.options.max_entries) {
            return;
        }
        if new.is_some() {
            self.divider = new;
        }
        let reads = self.reads();
        let conversation = &mut self.conversations[at];
        // The conversation shown counts too while nobody's looking at it.
        if unread && (at != self.selected || !reads) {
            conversation.unread += 1;
            if mention {
                conversation.mentions += 1;
            }
        }
        if let Some(entry) = copy {
            self.fresh.push((conversation.name.clone(), entry));
        }
    }

    /// The server's word that we're `away`, or not, said where the person is looking when
    /// they asked. A person is away only once each of their apps is (Ergo's auto-away), so
    /// asking to be away while another is connected is answered with "not away".
    pub(super) fn away_reply(&mut self, away: bool) {
        let asked = self.asked_away.take();
        if !away {
            self.away.remove(&fold(&self.me));
            match asked {
                Some(true) => self.note(
                    "You're away here, but shown as here while another of your apps is \
                     connected and not away."
                        .to_owned(),
                ),
                Some(false) => self.note("You're back.".to_owned()),
                None => {}
            }
            return;
        }
        let here = self.status.away_here.clone().unwrap_or_default();
        let message = self.away.entry(fold(&self.me)).or_insert(here).clone();
        if asked.is_some() {
            self.note(match message.as_str() {
                "" | "away" => "You're away.".to_owned(),
                why => format!("You're away: {why}"),
            });
        }
    }

    /// `name`'s members are all in (`RPL_ENDOFNAMES`), as on joining it or attaching to an
    /// account that's in it: a channel's machines are asked what they're working on, which
    /// no history keeps ([`crate::presence::ask`]). A server that keeps history, as zirc's
    /// does, is one whose clients take message tags.
    pub(super) fn names_done(&mut self, name: &str) -> Vec<Line> {
        if let Some(at) = self.position(name) {
            self.conversations[at].names_done = true;
        }
        if is_channel(name) && self.link.as_ref().is_some_and(|link| link.chathistory) {
            vec![crate::presence::ask(name)]
        } else {
            Vec::new()
        }
    }
}

/// What a PRIVMSG's or NOTICE's `body` is as an entry and the words it says
/// ([`entry_of`]), with a message's attached files and the body without their links, which
/// its text ends with for readers that don't know the tag ([`zirc_proto::attached`]).
fn said<'a>(
    line: &Line,
    body: &'a str,
    notice: bool,
) -> Option<(EntryKind, String, Vec<zirc_proto::File>, &'a str)> {
    let files = line
        .tag(zirc_proto::attached::TAG)
        .filter(|_| !notice)
        .map(zirc_proto::attached::parse)
        .unwrap_or_default();
    let body = zirc_proto::attached::words(body, &files);
    let (entry, words) = entry_of(body, notice)?;
    let files = if matches!(entry, EntryKind::Message(_)) {
        files
    } else {
        Vec::new()
    };
    Some((entry, words, files, body))
}
