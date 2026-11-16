//! Which conversation is shown, and what is read and unread in each.

use super::{Chat, Conversation, DateTime, Kind, Line, Reading, Utc, agent, fold, stamp};
use zirc_proto::push::{self, PUSHER};

impl Chat {
    /// The conversation shown.
    #[must_use]
    pub fn selected(&self) -> &Conversation {
        &self.conversations[self.selected]
    }

    /// Where the conversation shown is in [`Chat::conversations`].
    #[must_use]
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Shows the conversation at `at`, marking it read. Returns what to send: where it's
    /// been read up to, for the person's other apps.
    #[must_use = "it says where the conversation's been read up to"]
    pub fn select(&mut self, at: usize) -> Vec<Line> {
        if at >= self.conversations.len() {
            return Vec::new();
        }
        self.server_shown = false;
        self.divider = self.first_unread(&self.conversations[at].name.clone());
        // Opened again, a closed direct message is back in the sidebar.
        self.closed.remove(&fold(&self.conversations[at].name));
        self.selected = at;
        // Opened from anywhere, Jump to… or a notification, the sidebar shows its section.
        if self.conversations[at].kind != Kind::Server {
            self.tab = super::SidebarSection::of(&self.conversations[at].name);
        }
        self.reading = Reading::AtEnd;
        self.first_shown = false;
        let conversation = &mut self.conversations[at];
        conversation.unread = 0;
        conversation.mentions = 0;
        self.mark(at)
    }

    /// Shows the server's own conversation as the Server channel: what the server and its
    /// services said, where what's typed goes to the server as it is.
    pub fn show_server(&mut self) {
        if let Some(at) = self.position(&self.server.clone()) {
            // The server's conversation has nothing to mark read.
            let _ = self.select(at);
            self.server_shown = true;
        }
    }

    /// Whether the Server channel is shown ([`Chat::show_server`]).
    #[must_use]
    pub fn server_shown(&self) -> bool {
        self.server_shown
    }

    /// The id of the message the "New" divider goes above in the conversation shown, as
    /// Slack draws it: the first it hadn't read when it was shown, until it's marked read
    /// or another is shown.
    #[must_use]
    pub fn divider(&self) -> Option<&str> {
        self.divider.as_deref()
    }

    /// The New line came on screen, or the pill over the messages was closed: the pill has said
    /// its piece, and goes for good, not back as the person reads on down past the line. It
    /// shows again only for a newer divider, as what arrives while they're further up starts.
    pub fn saw_divider(&mut self) {
        self.divider_seen.clone_from(&self.divider);
    }

    /// Says whether the person is reading the conversation shown at its end. What arrives
    /// while they aren't is [`Chat::unseen`], from a New divider it starts.
    pub fn scrolled(&mut self, at_end: bool) {
        self.reading = if at_end { Reading::AtEnd } else { Reading::Up };
    }

    /// Says whether anyone is looking at the conversation shown: not while the app shows
    /// another organisation's, whose chat stays connected out of sight. Meanwhile what comes
    /// there counts as unread, as anywhere else, for the organisation's badge; looking again
    /// shows it as if just opened, from its New divider, marking it read. Returns what to send.
    #[must_use = "it says where the conversation's been read up to"]
    pub fn set_looking(&mut self, looking: bool) -> Vec<Line> {
        let was = self.reads();
        self.looking = looking;
        self.read_again(was)
    }

    /// Says whether the app is in front: its window the active one, or the phone's app in the
    /// foreground. Out of sight, nothing it shows is read, as on every app: what comes counts as
    /// unread, and coming back shows it from its New divider, marking it read, as decided ("Yes,
    /// and do both"). Returns what to send.
    #[must_use = "it says where the conversation's been read up to"]
    pub fn set_in_front(&mut self, in_front: bool) -> Vec<Line> {
        let was = self.reads();
        self.in_front = in_front;
        self.read_again(was)
    }

    /// Whether what the conversation shown says is read as it comes: it's shown, the app is in
    /// front, and the screen isn't locked.
    #[must_use]
    pub fn reads(&self) -> bool {
        self.looking && self.in_front && !self.locked
    }

    /// What to send once the person reads again, when they didn't (`was`): the conversation
    /// shown, as if just opened.
    pub(super) fn read_again(&mut self, was: bool) -> Vec<Line> {
        if self.reads() && !was {
            self.select(self.selected)
        } else {
            Vec::new()
        }
    }

    /// Shows the conversation `by` places on from the one shown, round the ends. Returns
    /// what to send, as [`Chat::select`] does.
    #[must_use = "it says where the conversation's been read up to"]
    pub fn cycle(&mut self, by: isize) -> Vec<Line> {
        let len = self.conversations.len().cast_signed();
        let at = (self.selected.cast_signed() + by).rem_euclid(len);
        self.select(at.cast_unsigned())
    }

    /// Marks the conversation called `name` read up to its newest message, as Escape does:
    /// here, and through the server in the person's other apps. Returns what to send.
    #[must_use = "it says where the conversation's been read up to"]
    pub fn mark_read(&mut self, name: &str) -> Vec<Line> {
        let Some(at) = self.position(name) else {
            return Vec::new();
        };
        if at == self.selected {
            self.divider = None;
        }
        let conversation = &mut self.conversations[at];
        conversation.unread = 0;
        conversation.mentions = 0;
        self.mark(at)
    }

    /// Marks every conversation read up to its newest message, as Shift+Escape does.
    /// Returns what to send.
    #[must_use = "it says where the conversations have been read up to"]
    pub fn mark_all_read(&mut self) -> Vec<Line> {
        self.divider = None;
        (0..self.conversations.len())
            .flat_map(|at| {
                let conversation = &mut self.conversations[at];
                conversation.unread = 0;
                conversation.mentions = 0;
                self.mark(at)
            })
            .collect()
    }

    /// Shows the next conversation that mentions us, or else the next with anything
    /// unread. What to send, as [`Chat::select`] returns, when there was one.
    #[must_use = "it says where the conversation's been read up to"]
    pub fn next_unread(&mut self) -> Option<Vec<Line>> {
        let len = self.conversations.len();
        let order = (1..len).map(|i| (self.selected + i) % len);
        let next = order
            .clone()
            .find(|&at| self.conversations[at].mentions > 0)
            .or_else(|| order.clone().find(|&at| self.conversations[at].unread > 0));
        next.map(|at| self.select(at))
    }

    /// Marks the conversation at `at` read up to its newest message, and tells the server,
    /// which tells the person's other apps.
    pub(super) fn mark(&mut self, at: usize) -> Vec<Line> {
        if !self.link.as_ref().is_some_and(|link| link.read_marker) {
            return Vec::new();
        }
        let conversation = &mut self.conversations[at];
        // The server keeps how far a machine's conversation was read, not each of its
        // agents'.
        if conversation.kind == Kind::Server || agent(&conversation.name).is_some() {
            return Vec::new();
        }
        let Some(time) = conversation.newest_timed() else {
            return Vec::new();
        };
        if !conversation.mark_read(time, &self.me) {
            return Vec::new();
        }
        let at = stamp(time);
        // The tenant's pusher hears it too, so a push waiting for what's now read isn't sent
        // to the person's phone, and one already sent goes from it (zirc_proto::push).
        let mut told = Line::new("TAGMSG", [PUSHER]);
        told.tags.push((
            push::TAG.to_owned(),
            Some(
                push::Said::Read {
                    target: conversation.name.clone(),
                    at: at.clone(),
                }
                .tag(),
            ),
        ));
        vec![
            Line::new(
                "MARKREAD",
                [conversation.name.clone(), format!("timestamp={at}")],
            ),
            told,
        ]
    }

    /// Asks the server how far the private conversation with `nick` has been read, which
    /// it only says of channels by itself. Not of a conversation with an agent: see
    /// [`Chat::mark`].
    pub(super) fn ask_read(&self, nick: &str) -> Option<Line> {
        self.link
            .as_ref()
            .is_some_and(|link| link.read_marker && agent(nick).is_none())
            .then(|| Line::new("MARKREAD", [nick]))
    }

    /// How far a conversation has been read, from the server: when we join a channel, ask
    /// about a private conversation, or another of the person's apps reads one.
    pub(super) fn read_marker(&mut self, line: &Line) -> Vec<Line> {
        let param = |i: usize| line.params.get(i).map_or("", String::as_str);
        let time = param(1)
            .strip_prefix("timestamp=")
            .and_then(|time| DateTime::parse_from_rfc3339(time).ok());
        if let (Some(at), Some(time)) = (self.position(param(0)), time) {
            self.conversations[at].mark_read(time.with_timezone(&Utc), &self.me);
        }
        Vec::new()
    }

    /// Whether `nick` is away, from WHO's flags: `G` for gone, `H` for here.
    pub(super) fn flagged(&mut self, nick: &str, flags: &str) {
        if flags.starts_with('G') {
            self.away.entry(fold(nick)).or_default();
        } else if flags.starts_with('H') {
            self.away.remove(&fold(nick));
        }
    }
}
