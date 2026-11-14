//! Who is here - people and their machines, agent sessions, who is a guest, and who is away.

use super::{
    BACKGROUND_AWAY, BTreeMap, Chat, DateTime, Entry, EntryKind, IDLE_AFTER_SECS, IDLE_AWAY, Kind,
    LOCKED_AWAY, Line, Message, Person, PersonRow, Presence, PresenceMap, SESSION_MARK, Utc, agent,
    away_line, conversation, fold, for_agent, is_guest, nick_of, people, view,
};

impl Chat {
    /// The people in the conversation called `name`, as every app names them: see
    /// [`Person`].
    #[must_use]
    pub fn people(&self, name: &str) -> Vec<Person> {
        self.conversation(name)
            .map_or_else(Vec::new, |conversation| {
                people::people(
                    conversation,
                    &self.me,
                    |nick| self.machine_owner(nick).map(str::to_owned),
                    |nick| self.away(nick).map(str::to_owned),
                    |nick| self.is_guest(nick),
                    |nick| self.shown_name(nick),
                    |nick| self.app_of(nick),
                )
            })
    }

    /// The people in the conversation called `name`, one row each, with their machines and
    /// those machines' agent sessions under them: `alice` and `alice · mbp` are one row. See
    /// [`PersonRow`].
    #[must_use]
    pub fn people_rows(&self, name: &str) -> Vec<PersonRow> {
        // zirc's apps have a part of their own: see [`Chat::app_rows`].
        let people = self
            .people(name)
            .into_iter()
            .filter(|person| person.app.is_none())
            .collect();
        let mut rows = people::rows(
            people,
            &self.me,
            |nick| self.away(nick).map(str::to_owned),
            |nick| self.shown_name(nick),
        );
        let sessions = self.sessions(|nick| {
            rows.iter()
                .flat_map(|row| &row.machines)
                .any(|machine| machine.person.nick.eq_ignore_ascii_case(nick))
        });
        for machine in rows.iter_mut().flat_map(|row| &mut row.machines) {
            let Some((_, machine_name)) = machine.person.machine.clone() else {
                continue;
            };
            machine.sessions = sessions
                .iter()
                .filter_map(|name| conversation::agent(name))
                .filter(|(nick, _)| nick.eq_ignore_ascii_case(&machine.person.nick))
                .filter(|(nick, session)| self.session_live(nick, session) != Some(false))
                .map(|(nick, session)| self.session_row(nick, session, machine_name.clone()))
                .collect();
        }
        // Who's here first, then who's away, then who's gone, each part in the order
        // [`people::people`] gives, as every app's people list has them.
        rows.sort_by_key(|row| match row.standing().presence() {
            Presence::Here => 0,
            Presence::Away => 1,
            Presence::Gone => 2,
        });
        rows
    }

    /// What `message` says of the agent session that sent it: the name it goes by, its tool
    /// and its picture from the gallery.
    pub(super) fn learn_session(&mut self, message: &Message) {
        let session = &message.session;
        if let Some(name) = message.name.as_deref().and_then(zirc_proto::session_name) {
            self.session_names.insert(session.clone(), name);
        }
        if let Some(tool) = &message.tool {
            self.session_tools.insert(session.clone(), tool.clone());
        }
        if let Some(picture) = message.avatar.as_deref().and_then(zirc_proto::agent_avatar) {
            self.session_pictures
                .insert(session.clone(), picture.to_owned());
        }
    }

    /// What to call the agent session `session`, such as one a message names (`cc-598aa1`):
    /// the name it goes by, `zirc-chat`, once one of its messages has said, or else the id.
    #[must_use]
    pub fn session_label(&self, session: &str) -> String {
        self.session_names
            .get(session)
            .cloned()
            .unwrap_or_else(|| session.to_owned())
    }

    /// Which of zirc's apps `nick` is, `alerts`, by its nick, `zirc-alerts`, or else by the
    /// account the server says it's logged in to, as before it's posted anything: `None`
    /// for anyone else. Every count of people leaves these out.
    #[must_use]
    pub fn app_of(&self, nick: &str) -> Option<String> {
        view::app_label(nick, None).or_else(|| {
            let account = self.accounts.get(&fold(nick))?.as_deref()?;
            view::app_label(account, None)
        })
    }

    /// Whether `nick` is one of zirc's apps ([`Chat::app_of`]).
    #[must_use]
    pub fn is_app(&self, nick: &str) -> bool {
        self.app_of(nick).is_some()
    }

    /// Whether `nick` is a guest's, by its nick and any account the server has said it's
    /// logged in to: see [`is_guest`].
    #[must_use]
    pub fn is_guest(&self, nick: &str) -> bool {
        is_guest(
            nick,
            self.accounts
                .get(&fold(nick))
                .map(|account| account.as_deref()),
        )
    }

    /// Who is changing which files, from the working sets heard live.
    #[must_use]
    pub fn presence(&self) -> &PresenceMap {
        &self.presence
    }

    /// `nick`'s IRC real name, once the server has said it: for a machine, its owner.
    #[must_use]
    pub fn realname(&self, nick: &str) -> Option<&str> {
        self.realnames.get(&fold(nick)).map(String::as_str)
    }

    /// Whether `nick` is a machine rather than a person: its host says whose it is.
    pub(super) fn is_machine(&self, nick: &str) -> bool {
        self.machine_owner(nick).is_some()
    }

    /// Whose machine `nick` is, its owner's handle, as its host says (`machine/<owner>`, which
    /// only the server sets); None for a person, or a machine whose host hasn't said.
    #[must_use]
    pub fn machine_owner(&self, nick: &str) -> Option<&str> {
        self.machines.get(&fold(nick)).map(String::as_str)
    }

    /// `nick`'s host, as a line from it or WHO says: whose machine it is, when it says.
    pub(super) fn hosted(&mut self, nick: &str, host: &str) {
        match zirc_proto::names::owner_from_host(host) {
            Some(owner) => {
                self.machines.insert(fold(nick), owner.to_owned());
            }
            None => {
                self.machines.remove(&fold(nick));
            }
        }
    }

    /// What WHOX says of `nick`: its flags, its account (`0` for none) and its real name.
    pub(super) fn whox(&mut self, nick: &str, flags: &str, account: &str, realname: &str) {
        self.realnames.insert(fold(nick), realname.to_owned());
        let account = Some(account).filter(|account| *account != "0");
        self.accounts.insert(fold(nick), account.map(str::to_owned));
        self.flagged(nick, flags);
    }

    /// The agent sessions on our own machines we can talk to, each as its conversation is
    /// named, `<machine>/<session>`, sorted: those working, taking something on or about
    /// to, in the channels we're in, and those we have a conversation with.
    #[must_use]
    pub fn agents(&self) -> Vec<String> {
        self.sessions(|nick| self.ours(nick))
    }

    /// Our own machines that are online, by nick, sorted: those in a channel we're in, as
    /// zircd is in its repositories' channels and `zirc-remote` in those its config names.
    /// A machine that joins no channel we're in isn't known to be online.
    #[must_use]
    pub fn our_machines(&self) -> Vec<String> {
        let mut machines: BTreeMap<String, String> = BTreeMap::new();
        for conversation in &self.conversations {
            if conversation.kind != Kind::Channel {
                continue;
            }
            for member in conversation.members.values() {
                let nick = &member.nick;
                if !nick.eq_ignore_ascii_case(&self.me)
                    && self.whose(nick).is_some()
                    && self.ours(nick)
                {
                    machines.entry(fold(nick)).or_insert_with(|| nick.clone());
                }
            }
        }
        machines.into_values().collect()
    }

    /// The agent session going by `handle`, as [`Chat::session_label`] calls it, or with that
    /// id, in the channels we're in or a conversation we have with it: its machine's nick and
    /// its id, for its card ([`Chat::agent_card`]).
    #[must_use]
    pub fn session_named(&self, handle: &str) -> Option<(String, String)> {
        self.sessions(|_| true).into_iter().find_map(|name| {
            let (nick, session) = agent(&name)?;
            (session.eq_ignore_ascii_case(handle)
                || self.session_label(session).eq_ignore_ascii_case(handle))
            .then(|| (nick.to_owned(), session.to_owned()))
        })
    }

    /// The agent sessions of the machines `keep` keeps, as [`Chat::agents`] finds ours:
    /// those live ([`Chat::session_live`]), those we have a conversation with, and, unless
    /// their machine says they've ended, those working, taking something on or about to.
    pub(super) fn sessions(&self, keep: impl Fn(&str) -> bool) -> Vec<String> {
        let now = u64::try_from((self.clock)().timestamp()).unwrap_or_default();
        let mut agents: BTreeMap<String, String> = BTreeMap::new();
        for name in self.live_sessions(&keep) {
            agents.entry(fold(&name)).or_insert(name);
        }
        let mut add = |nick: &str, session: &str, ended_too: bool| {
            if keep(nick)
                && !session.is_empty()
                && session != zirc_proto::NO_SESSION
                && (ended_too || self.session_live(nick, session) != Some(false))
            {
                let name = format!("{nick}{SESSION_MARK}{session}");
                agents.entry(fold(&name)).or_insert(name);
            }
        };
        for conversation in &self.conversations {
            let channel = &conversation.name;
            if let Some((nick, session)) = agent(channel) {
                add(nick, session, true);
            }
            if conversation.kind != Kind::Channel {
                continue;
            }
            for member in self.presence.members(channel) {
                if let Some(session) = &member.session {
                    add(&member.nick, session, false);
                }
            }
            for claim in self.presence.claims(channel) {
                add(&claim.nick, &claim.session, false);
            }
            for intent in self.presence.intents(channel, now) {
                add(&intent.nick, &intent.session, false);
            }
        }
        agents.into_values().collect()
    }

    /// Whose machine `nick` is and its own name, its nick, as its host says
    /// ([`super::Chat::machine_owner`]).
    pub(super) fn machine(&self, nick: &str) -> Option<(String, String)> {
        self.machine_owner(nick)
            .map(|owner| (owner.to_owned(), nick.to_owned()))
    }

    /// Whether `nick` is one of our own machines, whose agents talk to us.
    pub(super) fn ours(&self, nick: &str) -> bool {
        self.machine(nick)
            .is_some_and(|(owner, _)| owner.eq_ignore_ascii_case(&self.me))
    }

    /// The conversation with the agent `session` on our machine `nick`, made if there isn't
    /// one. What we said to that session before it was made, kept with the machine's own
    /// conversation, as history from before it answered, moves to it.
    pub(super) fn ensure_agent(&mut self, nick: &str, session: &str) -> usize {
        let name = format!("{nick}{SESSION_MARK}{session}");
        if let Some(at) = self.position(&name) {
            return at;
        }
        let at = self.ensure(&name, Kind::Query);
        let Some(plain) = self.position(nick) else {
            return at;
        };
        let me = self.me.clone();
        let taken = self.conversations[plain].take(|entry| {
            entry.from.eq_ignore_ascii_case(&me)
                && matches!(&entry.kind, EntryKind::Message(text) if for_agent(text, session).is_some())
        });
        let max = self.options.max_entries;
        for mut entry in taken {
            if let EntryKind::Message(text) = &entry.kind
                && let Some(rest) = for_agent(text, session)
            {
                entry.kind = EntryKind::Message(rest.to_owned());
            }
            self.conversations[at].insert(entry, max);
        }
        at
    }

    /// Why `nick` is away, when they are: their away message, which may be empty. For us
    /// too, only as the server says: a person is away once each of their apps is (Ergo's
    /// auto-away), so asking from one while another is connected leaves them here, and
    /// [`Status::away_here`] says what this app asked. A person's account stays on the
    /// server while none of their apps is connected, and is away then.
    #[must_use]
    pub fn away(&self, nick: &str) -> Option<&str> {
        self.away.get(&fold(nick)).map(String::as_str)
    }

    /// Says this app is away, with `message`, or with `None` that it's back. Returns what to
    /// send. [`Status::away_here`] says it at once; [`Chat::away`] once the server agrees.
    pub fn set_away(&mut self, message: Option<&str>) -> Vec<Line> {
        self.auto_away = false;
        self.asked_away = Some(message.is_some());
        self.status.away_here = message.map(|message| {
            if message.is_empty() {
                super::BARE_AWAY
            } else {
                message
            }
            .to_owned()
        });
        away_line(self.status.away_here.as_deref())
    }

    /// Under an app's own Away switch, when it said we're away but the server still shows us
    /// here, because another of our apps is connected and not away; none otherwise.
    #[must_use]
    pub fn away_note(&self) -> Option<&'static str> {
        (self.status.away_here.is_some() && self.away(&self.me).is_none())
            .then_some("Shown as here while another of your apps is connected and not away.")
    }

    /// The computer's state, from the app that sees it: seconds since the last input
    /// anywhere on it, and whether the screen is locked. Idle for [`IDLE_AFTER_SECS`], or
    /// locked, this app goes away on its own, saying [`IDLE_AWAY`], and comes back at the
    /// next input. Away by hand stays as the person asked, whatever this says. Returns what
    /// to send, which is nothing while nothing changes; call it as often as is cheap.
    pub fn set_idle(&mut self, idle_secs: u64, locked: bool) -> Vec<Line> {
        let why = if locked { LOCKED_AWAY } else { IDLE_AWAY };
        // Nothing's read behind a locked screen ([`Chat::reads`]).
        let was = self.reads();
        self.locked = locked;
        let mut lines = self.go_away_by_itself(locked || idle_secs >= IDLE_AFTER_SECS, why);
        lines.extend(self.read_again(was));
        lines
    }

    /// A phone's app went to the background, or came back in front: away by itself,
    /// saying [`BACKGROUND_AWAY`], and here again in front. Away by hand stays as asked.
    /// Returns what to send.
    pub fn set_background(&mut self, background: bool) -> Vec<Line> {
        self.go_away_by_itself(background, BACKGROUND_AWAY)
    }

    /// Away by itself, saying `why`, when `away` and this app isn't away already; back when
    /// it isn't and it went away by itself.
    fn go_away_by_itself(&mut self, away: bool, why: &str) -> Vec<Line> {
        if away && self.status.away_here.is_none() {
            self.auto_away = true;
            self.status.away_here = Some(why.to_owned());
            return away_line(self.status.away_here.as_deref());
        }
        if !away && self.auto_away {
            self.auto_away = false;
            self.status.away_here = None;
            return away_line(None);
        }
        Vec::new()
    }

    /// The messages that arrived live since the last call, oldest first, with their
    /// conversation's name: what might notify.
    pub fn take_fresh(&mut self) -> Vec<(String, Entry)> {
        std::mem::take(&mut self.fresh)
    }

    /// The ids of the messages in `name` whose reactions changed since the last call.
    pub fn take_touched(&mut self, name: &str) -> Vec<String> {
        self.position(name)
            .map(|at| self.conversations[at].take_touched())
            .unwrap_or_default()
    }

    /// `old` is now called `new`, in every conversation they're in.
    pub(super) fn renamed(&mut self, old: &str, new: &str, time: DateTime<Utc>) {
        if old.eq_ignore_ascii_case(&self.me) {
            new.clone_into(&mut self.me);
            self.status.notice(time, "", format!("you are now {new}"));
        }
        if let Some(account) = self.accounts.remove(&fold(old)) {
            self.accounts.insert(fold(new), account);
        }
        if let Some(realname) = self.realnames.remove(&fold(old)) {
            self.realnames.insert(fold(new), realname);
        }
        if let Some(owner) = self.machines.remove(&fold(old)) {
            self.machines.insert(fold(new), owner);
        }
        if let Some(message) = self.away.remove(&fold(old)) {
            self.away.insert(fold(new), message);
        }
        self.presence.renamed(old, new);
        for at in 0..self.conversations.len() {
            let conversation = &mut self.conversations[at];
            let prefix = conversation.members.get(&fold(old)).map(|m| m.prefix);
            if let Some(prefix) = prefix {
                conversation.remove_member(old);
                let shown: String = prefix.into_iter().chain(new.chars()).collect();
                conversation.add_member(&shown);
            } else if conversation.kind == Kind::Query && conversation.is(old) {
                new.clone_into(&mut conversation.name);
                self.reindex();
            } else {
                continue;
            }
            let entry = Entry {
                from: old.to_owned(),
                owner: None,
                kind: EntryKind::Nick(new.to_owned()),
                ..Entry::info(time, String::new())
            };
            self.push(at, entry);
        }
    }

    /// Someone went away, saying why, or came back (`away-notify`).
    pub(super) fn away_changed(&mut self, line: &Line) -> Vec<Line> {
        let nick = fold(nick_of(line));
        match line.params.first() {
            Some(message) if !message.is_empty() => {
                // They were last active when they went, or ten minutes before an app says
                // they were idle that long. Told again, it keeps the first time.
                if self.away.get(&nick) != Some(message) {
                    let at = self.time_of(line);
                    let at = if message == IDLE_AWAY {
                        at - chrono::Duration::seconds(
                            i64::try_from(IDLE_AFTER_SECS).unwrap_or_default(),
                        )
                    } else {
                        at
                    };
                    self.last_active.entry(nick.clone()).or_insert(at);
                }
                self.away.insert(nick, message.clone());
            }
            _ => {
                self.away.remove(&nick);
                self.last_active.remove(&nick);
            }
        }
        Vec::new()
    }
}
