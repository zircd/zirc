//! The connection - registering, losing it and coming back, and taking in each line the server sends.

use super::{
    Attendance, Batches, Chat, Connection, Kind, Line, Link, Status, away_line, fold, names,
    nick_of, stamp, target,
};

impl Chat {
    /// What the connection allows, while connected.
    #[must_use]
    pub fn link(&self) -> Option<&Link> {
        self.link.as_ref()
    }

    /// Whether we're connected.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.link.is_some()
    }

    /// The connection, and what the server and its services said, for a status bar. None
    /// of it shows in a conversation.
    #[must_use]
    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Registration finished: ask for the channels, which get joined once listed, and for
    /// the private conversations with history.
    pub fn connected(&mut self, link: Link) -> Vec<Line> {
        self.me.clone_from(&link.nick);
        // What was last known offline goes as the server says it afresh: who's away by WHO,
        // agents' work as they say it again and as history replays it.
        self.away.clear();
        self.last_active.clear();
        self.presence.clear();
        self.live_unsure((self.clock)());
        self.listing.clear();
        self.history.clear();
        self.targets.clear();
        self.gaps.clear();
        self.filling.clear();
        self.multiline = Batches::default();
        let chathistory = link.chathistory;
        let metadata = link.metadata;
        self.link = Some(link);
        self.listed_once = false;
        self.asked.clear();
        for conversation in &mut self.conversations {
            if conversation.kind == Kind::Channel {
                if conversation.joined {
                    conversation.standing = Attendance::Rejoin;
                }
                conversation.joined = false;
            }
            conversation.members.clear();
        }
        self.status.connection = Connection::Connected;

        let mut lines = if self.options.list_channels {
            vec![Line::new("LIST", [""; 0])]
        } else {
            let only = self.joins.clone();
            self.join(&only)
        };
        // What was opened while offline is joined now, ahead of the list: our doing, so a
        // server that keeps our channels doesn't take it for one it put us back in, and the
        // latest opened is shown once joined.
        let offline = std::mem::take(&mut self.offline_joins);
        if !offline.is_empty() {
            let joining = self.join(&offline);
            lines.splice(0..0, joining);
        }
        if metadata {
            lines.push(Line::new(
                "METADATA",
                ["*", "SUB", names::NAME_KEY, names::AVATAR_KEY],
            ));
        }
        // The server forgets a connection's away with it: this app is away again.
        if self.status.away_here.is_some() {
            self.asked_away = Some(true);
            lines.extend(away_line(self.status.away_here.as_deref()));
        }
        if chathistory {
            // Private conversations with history, from the last month.
            let now = (self.clock)();
            let since = now - super::history::TARGETS_SPAN;
            lines.push(Line::new(
                "CHATHISTORY",
                [
                    "TARGETS".to_owned(),
                    format!("timestamp={}", stamp(since)),
                    format!("timestamp={}", stamp(now)),
                    "50".to_owned(),
                ],
            ));
            let mut queries: Vec<String> = Vec::new();
            for conversation in &self.conversations {
                let name = target(&conversation.name);
                if conversation.kind == Kind::Query
                    && !queries.iter().any(|q| q.eq_ignore_ascii_case(name))
                {
                    queries.push(name.to_owned());
                }
            }
            for name in queries {
                lines.push(self.catch_up(&name));
            }
        }
        // The server says how far each channel's been read as we join it; private
        // conversations we ask about.
        for conversation in &self.conversations {
            if conversation.kind == Kind::Query {
                lines.extend(self.ask_read(&conversation.name));
            }
        }
        // Direct messages said while offline go now; a channel's once it's joined again.
        lines.extend(self.release(None));
        lines
    }

    /// Connecting again, after the connection was lost.
    pub fn connecting(&mut self) {
        self.status.connection = Connection::Connecting;
    }

    /// The connection ended. Who was where, who's away and what agents are at stay as they
    /// were last known, [`Chat::is_stale`], for apps to dim rather than empty: the server
    /// says them afresh on connecting again, each channel's people by its `NAMES`.
    pub fn disconnected(&mut self, why: &str) {
        self.link = None;
        // The MOTD says it again on connecting, while it's still so.
        self.status.logged = None;
        self.status.connection = Connection::Disconnected(why.to_owned());
        self.history.clear();
        self.gaps.clear();
        self.filling.clear();
        self.asked_away = None;
        self.working_at.clear();
        for conversation in &mut self.conversations {
            if conversation.kind == Kind::Channel {
                if conversation.joined {
                    conversation.standing = Attendance::Rejoin;
                }
                conversation.joined = false;
            }
            // Whoever's next NAMES says replaces them all ("353").
            conversation.names_done = true;
        }
    }

    /// Whether who's here, who's away, the counts and the agents at work are as last known,
    /// not as the server says now: while not connected. Apps dim them, never empty them.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        !self.is_connected()
    }

    /// Time passed: ask again which channels there are, to join new ones.
    #[must_use]
    pub fn tick(&self) -> Vec<Line> {
        if self.is_connected() && self.options.list_channels {
            vec![Line::new("LIST", [""; 0])]
        } else {
            Vec::new()
        }
    }

    /// Takes in a line from the server, saying what to send back. The lines of a
    /// multiline message are held until it ends, then taken in as one.
    pub fn receive(&mut self, line: Line) -> Vec<Line> {
        let Some(line) = self.multiline.feed(line) else {
            return Vec::new();
        };
        self.server_window(&line);
        // account-tag: whoever's line carries an account is logged in to it.
        if line
            .tag("account")
            .is_some_and(|account| !account.is_empty())
            && line
                .source
                .as_deref()
                .is_some_and(|source| source.contains('!'))
        {
            let account = line.tag("account").map(str::to_owned);
            self.accounts.insert(fold(nick_of(&line)), account);
        }
        let history = line
            .tag("batch")
            .is_some_and(|batch| self.history.contains(batch));
        // Whose machine the sender is, as its host says now: history's hosts may be old.
        if !history
            && let Some((nick, host)) = line
                .source
                .as_deref()
                .and_then(|source| source.split_once('!'))
                .and_then(|(nick, rest)| Some((nick, rest.split_once('@')?.1)))
        {
            self.hosted(nick, host);
        }
        if let Some(page) = line
            .tag("batch")
            .and_then(|batch| self.filling.get_mut(batch))
            && let Some(id) = line.tag("msgid")
        {
            page.count += 1;
            // History comes oldest first.
            page.oldest.get_or_insert_with(|| id.to_owned());
            page.newest = Some(id.to_owned());
            page.reached |= page.since.as_deref() == Some(id);
        }
        if let Some(batch) = line.tag("batch")
            && let Some(id) = line.tag("msgid")
            && self.filling.get(batch).is_some_and(|page| !page.known)
        {
            let target = self.filling[batch].target.clone();
            let known = self
                .conversations
                .iter()
                .filter(|c| super::target(&c.name).eq_ignore_ascii_case(&target))
                .any(|c| c.has_entry(id));
            if let Some(page) = self.filling.get_mut(batch) {
                page.known = known;
            }
        }
        let mut lines = match line.command.as_str() {
            "PRIVMSG" | "NOTICE" => {
                let mut lines = self.message(&line, history);
                // A part sealed with a key this device lacks: it asks for the key.
                lines.extend(self.ask_for_sealed(&line));
                lines
            }
            "TAGMSG" => self.tagmsg(&line),
            "BATCH" | "CHATHISTORY" => self.batch(&line),
            "JOIN" | "PART" | "KICK" | "QUIT" | "NICK" | "TOPIC" => self.membership(&line, history),
            // chghost: a host set while the nick's on, such as a machine's, which says whose.
            "CHGHOST" => {
                if let Some(host) = line.params.get(1) {
                    let nick = super::nick_of(&line).to_owned();
                    self.hosted(&nick, host);
                }
                Vec::new()
            }
            "MARKREAD" => self.read_marker(&line),
            // What the server supports, its network's name among it: the organisation's slug.
            "005" => {
                if let Some(network) = line
                    .params
                    .iter()
                    .find_map(|param| param.strip_prefix("NETWORK="))
                {
                    self.network = Some(network.to_owned());
                }
                Vec::new()
            }
            "REDACT" => self.trimmed(&line),
            "AWAY" => self.away_changed(&line),
            // setname: someone's real name changed, as when their profile's name did.
            "SETNAME" => {
                if let Some(name) = line.params.first() {
                    self.realnames.insert(fold(nick_of(&line)), name.clone());
                }
                Vec::new()
            }
            "INVITE" => self.invited(&line, history),
            // A key followed: METADATA <target> <key> <visibility> [value], or as the server
            // answers asking, 761 <me> <target> <key> <visibility> <value>.
            "METADATA" => self.metadata(&line.params),
            "761" => self.metadata(line.params.get(1..).unwrap_or_default()),
            _ => self.reply(&line),
        };
        // What arrives in the conversation shown is read, while someone's looking at it.
        // History is marked once it's all in, not line by line.
        if self.history.is_empty() && self.reads() {
            lines.extend(self.mark(self.selected));
        }
        lines
    }
}
