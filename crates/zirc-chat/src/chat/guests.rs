//! Guests, as every app shows them to an organisation's staff: visitors a public channel let
//! in without being members ([`Chat::is_guest`]), each talked to in their room of their own
//! beside it (`#support/guest-21042`, [`zirc_proto::guest_room`]), which the team shares.
//!
//! Who it's assigned to and whether it's closed are [`zirc_proto::support`] events said in
//! the room, which the server keeps with it, folded here like any other: there's no store of
//! their own. The room's row under its channel shows them ([`super::GuestRoom`]) and its
//! header says them ([`Chat::guest_header`]); agents never see them, and are never notified
//! of what a guest says.

use zirc_line::Line;
use zirc_proto::support::{self, Support};

use super::conversation::{Entry, EntryKind, Kind, fold, is_channel};
use super::{Chat, Level, is_group};

/// What apps say above the composer in a conversation with a guest.
pub const COMPOSER_NOTE: &str = "You're writing to a guest outside your organisation.";

/// The metadata key the guest gateway sets on a guest: `1` when they're signed in to a
/// zirc.dev account, `0` when they aren't.
pub const SIGNED_IN_KEY: &str = "zirc.dev/signed-in";

/// The pill apps show beside a guest's name.
pub const GUEST_PILL: &str = "GUEST";

/// What a conversation with a guest's header says of where they're from.
pub const OUTSIDE: &str = "outside your organisation";

/// The real name the guest gateway gives a guest it knows no name for, which is no name.
/// (The controller's gateway says the same, as `gateway::REALNAME`.)
pub const GATEWAY_REALNAME: &str = "zirc.dev guest";

pub use zirc_proto::guest_label;

/// What a support event says as a system row: `mattias opened this conversation`. `guest`
/// is the guest's name, for the email they left.
#[must_use]
pub fn support_text(from: &str, event: &Support, guest: &str) -> String {
    match event {
        Support::Opened => format!("{from} opened this conversation"),
        Support::Assigned(to) => format!("{from} assigned this to {to}"),
        Support::Closed => format!("{from} closed this"),
        Support::Email(_) => format!("{guest} left an email"),
        Support::Joined(who) => format!("{who} joined"),
    }
}

/// Who a conversation with a guest is assigned to, as its row and header say it:
/// `assigned to dana`.
#[must_use]
pub fn assigned_label(who: &str) -> String {
    format!("assigned to {who}")
}

/// What a conversation with a guest's header shows, tinted for outside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestHeader {
    /// The guest's nick.
    pub nick: String,
    /// Their name as shown: see [`Chat::guest_name`].
    pub label: String,
    /// Whether they're signed in to a zirc.dev account, once that's known
    /// ([`Chat::set_guest_account`]).
    pub account: Option<bool>,
    /// Who it's assigned to, by nick.
    pub assignee: Option<String>,
    /// Who opened it, by nick.
    pub opened_by: Option<String>,
    /// The email they left, confirmed.
    pub email: Option<String>,
    /// Whether it's closed.
    pub closed: bool,
}

impl GuestHeader {
    /// The line under the guest's name, as every app's header says it: their account, that
    /// they're outside, and who it's assigned to, `no account · outside your organisation
    /// · assigned to dana`.
    #[must_use]
    pub fn about(&self) -> String {
        let assigned = self.assignee.as_deref().map(assigned_label);
        [
            self.account_line().map(str::to_owned),
            Some(OUTSIDE.to_owned()),
            assigned,
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
    }

    /// What it says of their account: `has a zirc.dev account` or `no account`.
    #[must_use]
    pub fn account_line(&self) -> Option<&'static str> {
        self.account.map(|account| {
            if account {
                "has a zirc.dev account"
            } else {
                "no account"
            }
        })
    }

    /// What staff can do from it: an admin can also ban.
    #[must_use]
    pub fn actions(&self, admin: bool) -> Vec<GuestAction> {
        let mut actions = vec![GuestAction::Assign];
        if !self.closed {
            actions.push(GuestAction::Close);
        }
        // An owner or admin may also refuse them, lift that, and invite them in.
        if admin {
            actions.extend([GuestAction::Ban, GuestAction::Unban, GuestAction::Invite]);
        }
        actions
    }
}

/// Something staff can do with a conversation with a guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum GuestAction {
    /// Assign it to someone on staff: [`Chat::assign_guest`].
    Assign,
    /// Mark it resolved: [`Chat::close_guest`].
    Close,
    /// Refuse them in every public channel, which the organisation's server does, for admins.
    Ban,
    /// Lift their ban, for admins: [`Chat::unban_guest`].
    Unban,
    /// Invite them to be a member, which zirc.dev sends them, for admins:
    /// [`Chat::invite_guest`].
    Invite,
}

impl GuestAction {
    /// Its label, as the header's buttons and menu say.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Assign => "Assign to…",
            Self::Close => "Close",
            Self::Ban => "Ban",
            Self::Unban => "Unban",
            Self::Invite => "Invite as member",
        }
    }
}

/// A request an app makes of zirc.dev's Guests API for what isn't said over IRC: a ban,
/// which the organisation's server enforces, or forgetting a guest at once. Admins only.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
pub struct GuestRequest {
    /// The HTTP method.
    pub method: &'static str,
    /// The path, under the web app's address.
    pub path: String,
}

/// The Guests API's path for the guest `nick` of the organisation `org` (its slug).
fn guest_path(org: &str, nick: &str) -> String {
    zirc_proto::irc::guest_path(org, nick)
}

/// What the events and lines of a conversation with a guest add up to.
#[derive(Debug, Default)]
struct State {
    opened_by: Option<String>,
    assignee: Option<String>,
    closed: bool,
    email: Option<String>,
    /// Whether the guest spoke last.
    unanswered: bool,
}

fn state(entries: &[Entry], guest: &str) -> State {
    let mut state = State::default();
    for entry in entries {
        match &entry.kind {
            EntryKind::Support(event) => {
                match event {
                    Support::Opened => {
                        state.opened_by.get_or_insert_with(|| entry.from.clone());
                    }
                    Support::Assigned(to) => state.assignee = Some(to.clone()),
                    Support::Closed => state.closed = true,
                    Support::Email(email) => state.email = Some(email.clone()),
                    Support::Joined(_) => {}
                }
                continue;
            }
            EntryKind::Message(_) | EntryKind::Notice(_) | EntryKind::Action(_) => {}
            _ => continue,
        }
        let theirs = entry.from.eq_ignore_ascii_case(guest);
        if theirs {
            // What a guest says opens a closed conversation again.
            state.closed = false;
        }
        state.unanswered = theirs;
    }
    state
}

impl Chat {
    /// Whether the conversation called `name` is with a guest.
    #[must_use]
    pub fn is_guest_conversation(&self, name: &str) -> bool {
        !is_channel(name) && self.is_guest(name)
    }

    /// The guest the conversation called `name` is with, for staff: the guest of a guest's
    /// room, where they're talked to, or of an old private conversation with one. What its
    /// composer warns of, and what Assign, Close and Ban act on.
    #[must_use]
    pub fn guest_of(&self, name: &str) -> Option<String> {
        if !self.is_staff() {
            return None;
        }
        match zirc_proto::guest_room_of(name) {
            Some((_, guest)) => Some(guest.to_lowercase()),
            None => self.is_guest_conversation(name).then(|| name.to_owned()),
        }
    }

    /// Where Message on `nick`'s card takes us: for staff, a guest's room, where they're
    /// talked to (opening it joins it); else the direct message with them.
    #[must_use]
    pub fn message_target(&self, nick: &str) -> String {
        if self.is_staff() && self.is_guest(nick) {
            let room = self.support_room(nick).or_else(|| {
                self.guest_channel(nick)
                    .map(|channel| zirc_proto::guest_room(&channel, nick))
            });
            if let Some(room) = room {
                return room;
            }
        }
        nick.to_owned()
    }

    /// The public channel beside which `name` is our own room, if we're the guest it's for
    /// (`#support/guest-21042`, [`zirc_proto::guest_room`]): what we see it as, as nobody but
    /// us and the team is in it.
    #[must_use]
    pub fn our_room_beside(&self, name: &str) -> Option<String> {
        zirc_proto::guest_room_of(name)
            .filter(|(_, guest)| guest.eq_ignore_ascii_case(&self.me))
            .map(|(parent, _)| parent.to_lowercase())
    }

    /// Whether we're on an organisation's staff, who see conversations with guests: a
    /// person, neither a guest nor a machine.
    #[must_use]
    pub fn is_staff(&self) -> bool {
        !self.is_guest(&self.me) && !self.is_machine(&self.me)
    }

    /// A guest's name as staff see it: the name they're known by, which for someone signed
    /// in is their profile's and otherwise what they typed (the real name the gateway gives
    /// them); else the email they left; else `Guest 1a2b`. Never their nick.
    #[must_use]
    pub fn guest_name(&self, nick: &str) -> String {
        if let Some(name) = self
            .realname(nick)
            .map(str::trim)
            .filter(|name| !name.is_empty() && *name != GATEWAY_REALNAME)
        {
            return name.to_owned();
        }
        self.conversation(nick)
            .and_then(|c| state(&c.entries, nick).email)
            .unwrap_or_else(|| guest_label(nick))
    }

    /// Says whether we're one of the organisation's owners or admins, as zirc.dev tells the
    /// app: they can ban guests. Until it's told, nobody is.
    pub fn set_admin(&mut self, admin: bool) {
        self.admin = admin;
    }

    /// Sets [`Chat::set_admin`] from our role in the organisation, as zirc.dev's `/api/me`
    /// gives it: `owner`, `admin` or `member`.
    pub fn set_role(&mut self, role: &str) {
        let role = zirc_proto::role::Role::named(role);
        self.admin = role.is_some_and(zirc_proto::role::Role::is_admin);
        self.owner = role == Some(zirc_proto::role::Role::Owner);
    }

    /// Whether we're one of the organisation's owners or admins: see [`Chat::set_admin`].
    #[must_use]
    pub fn is_admin(&self) -> bool {
        self.admin
    }

    /// Whether apps show us `card`'s button `action`, as `docs/cards.md` says who may press
    /// it: `members` the organisation's, not guests; `anyone` everyone; `about` whoever
    /// the card is about (the owner of its `about` session, or that nick); `admins` owners
    /// and admins; `owners` owners alone. That only spares a pointless press; the
    /// integration decides.
    #[must_use]
    pub fn shows_action(
        &self,
        card: &zirc_proto::card::Card,
        action: &zirc_proto::card::Action,
    ) -> bool {
        use zirc_proto::card::Who;
        match action.who {
            Who::Anyone => true,
            Who::Members => !self.is_guest(&self.me),
            Who::Admins => self.admin,
            Who::Owners => self.owner,
            Who::About => card
                .about
                .as_deref()
                .is_some_and(|about| self.is_about_us(about)),
        }
    }

    /// Whether `about`, a card's agent session or nick, is us: a session one of our
    /// machines has spoken as, our nick, or one of our machines.
    fn is_about_us(&self, about: &str) -> bool {
        let ours = |machine: &str| {
            self.machine_owner(machine)
                .is_some_and(|owner| fold(owner) == fold(&self.me))
        };
        let spoke_as = self
            .conversations
            .iter()
            .flat_map(|conversation| &conversation.entries)
            .find_map(|entry| match &entry.kind {
                EntryKind::Zirc(message) if message.session.eq_ignore_ascii_case(about) => {
                    Some(entry.from.as_str())
                }
                _ => None,
            });
        match spoke_as {
            Some(machine) => ours(machine),
            None => fold(about) == fold(&self.me) || ours(about),
        }
    }

    /// Who a conversation with a guest can be assigned to: the people on staff in our
    /// channels, us included, operators first, then by name. Never a guest, a machine, an
    /// agent or one of zirc's apps.
    #[must_use]
    pub fn staff(&self) -> Vec<super::Person> {
        let mut staff: Vec<super::Person> = Vec::new();
        let channels = self
            .conversations
            .iter()
            .filter(|c| c.kind == Kind::Channel && !is_group(&c.name));
        for person in channels.flat_map(|c| self.people(&c.name)) {
            if person.guest || person.machine.is_some() || person.app.is_some() {
                continue;
            }
            match staff
                .iter_mut()
                .find(|known| known.nick.eq_ignore_ascii_case(&person.nick))
            {
                Some(known) => known.op |= person.op,
                None => staff.push(person),
            }
        }
        staff.retain(|person| self.is_staff_member(&person.nick));
        staff.sort_by_key(|person| (!person.op, fold(&person.nick)));
        staff
    }

    /// Whether `nick` is a person on staff: neither a guest nor a machine.
    fn is_staff_member(&self, nick: &str) -> bool {
        !self.is_guest(nick) && !self.is_machine(nick)
    }

    /// What staff can do from the header of the guest `nick`'s room (or the room called
    /// `nick`), as [`GuestHeader::actions`] says for us.
    #[must_use]
    pub fn guest_actions(&self, nick: &str) -> Vec<GuestAction> {
        self.support_room(nick)
            .and_then(|room| self.guest_header(&room))
            .map(|header| header.actions(self.admin))
            .unwrap_or_default()
    }

    /// The request that bans the guest `nick` from every public channel of the organisation
    /// `org` (its slug): `None` unless we're an admin.
    #[must_use]
    pub fn ban_guest(&self, org: &str, nick: &str) -> Option<GuestRequest> {
        self.admin_request(org, nick, "POST", "/ban")
    }

    /// The request that lifts the guest `nick`'s ban in the organisation `org`: `None`
    /// unless we're an admin.
    #[must_use]
    pub fn unban_guest(&self, org: &str, nick: &str) -> Option<GuestRequest> {
        self.admin_request(org, nick, "DELETE", "/ban")
    }

    /// The request that invites the guest `nick` to be a member of the organisation `org`,
    /// which zirc.dev sends to their account or confirmed email, so staff never see it:
    /// `None` unless we're an admin.
    #[must_use]
    pub fn invite_guest(&self, org: &str, nick: &str) -> Option<GuestRequest> {
        self.admin_request(org, nick, "POST", "/invite")
    }

    /// The request that forgets the guest `nick` of the organisation `org` at once: their
    /// account, conversations and email. `None` unless we're an admin.
    #[must_use]
    pub fn forget_guest(&self, org: &str, nick: &str) -> Option<GuestRequest> {
        self.admin_request(org, nick, "DELETE", "")
    }

    /// Says whether the guest `nick` is signed in to a zirc.dev account, as the
    /// organisation's server tells the app.
    pub fn set_guest_account(&mut self, nick: &str, account: bool) {
        self.guest_accounts.insert(fold(nick), account);
    }

    /// Metadata the server tells, as `target`, `key`, visibility and maybe a value: whether a
    /// guest is signed in to zirc.dev ([`SIGNED_IN_KEY`]), or how someone shows (see
    /// `names`).
    pub(super) fn metadata(&mut self, params: &[String]) -> Vec<Line> {
        if let [target, key, _visibility, rest @ ..] = params {
            let value = rest.first().map(String::as_str);
            if key == SIGNED_IN_KEY {
                self.set_guest_account(target, value == Some("1"));
            } else {
                self.profile_metadata(target, key, value);
            }
        }
        Vec::new()
    }

    /// The public channel the guest `nick` came from: one they're in, or else the last
    /// they spoke in.
    #[must_use]
    pub fn guest_channel(&self, nick: &str) -> Option<String> {
        let channels = || {
            self.conversations
                .iter()
                .filter(|c| c.kind == Kind::Channel && !is_group(&c.name))
        };
        channels()
            .find(|c| c.has_member(nick))
            .or_else(|| {
                channels()
                    .filter_map(|c| {
                        let said = c
                            .entries
                            .iter()
                            .rev()
                            .find(|e| e.from.eq_ignore_ascii_case(nick))?;
                        Some((said.time, c))
                    })
                    .max_by_key(|(time, _)| *time)
                    .map(|(_, c)| c)
            })
            .map(|c| c.name.clone())
    }

    /// The header of the guest's room called `name` (`#support/guest-21042`), as staff see
    /// it: who the guest is, who it's assigned to and what can be done; `None` for anything
    /// else. A guest's room is their one home: a private conversation with a guest is a
    /// direct message like any other.
    #[must_use]
    pub fn guest_header(&self, name: &str) -> Option<GuestHeader> {
        let (_, nick) = zirc_proto::guest_room_of(name).filter(|_| self.is_staff())?;
        let state = self.room_state(name).unwrap_or_default();
        Some(GuestHeader {
            nick: nick.to_lowercase(),
            // As their row under the channel names them, when it has a name for them.
            label: Some(self.guest_room_label(nick))
                .filter(|label| !label.eq_ignore_ascii_case(nick))
                .unwrap_or_else(|| self.guest_name(nick)),
            account: self.guest_accounts.get(&fold(nick)).copied(),
            assignee: state.assignee,
            opened_by: state.opened_by,
            email: state.email,
            closed: state.closed,
        })
    }

    /// What the support events and lines of the guest's room called `room` add up to.
    fn room_state(&self, room: &str) -> Option<State> {
        let (_, guest) = zirc_proto::guest_room_of(room)?;
        Some(state(&self.conversation(room)?.entries, guest))
    }

    /// Whether the guest's room called `room` is answered, assigned to us and closed, as its
    /// row shows them ([`super::GuestRoom`]): `(unanswered, mine, closed)`.
    pub(super) fn room_marks(&self, room: &str) -> (bool, bool, bool) {
        let state = self.room_state(room).unwrap_or_default();
        let mine = state
            .assignee
            .as_deref()
            .is_some_and(|nick| nick.eq_ignore_ascii_case(&self.me));
        (state.unanswered && !state.closed, mine, state.closed)
    }

    /// The guest's room that support events about `name` go to: `name` itself when it's a
    /// guest's room, or the room we're in of the guest called `name`.
    fn support_room(&self, name: &str) -> Option<String> {
        if zirc_proto::guest_room_of(name).is_some() {
            return Some(name.to_owned());
        }
        self.conversations
            .iter()
            .find(|c| {
                c.kind == Kind::Channel
                    && c.joined
                    && zirc_proto::guest_room_of(&c.name)
                        .is_some_and(|(_, guest)| guest.eq_ignore_ascii_case(name))
            })
            .map(|c| c.name.clone())
    }

    /// The lines that assign the guest `nick`'s room (or the room called `nick`) to `to`, one
    /// of [`Chat::staff`], who is then notified of all they say there, and tell the guest `to`
    /// joined. Said in the room, which the server keeps them in.
    #[must_use]
    pub fn assign_guest(&self, nick: &str, to: &str) -> Vec<Line> {
        // Only to a person on staff: never an agent, whom guests don't talk to.
        let Some(room) = self.support_room(nick) else {
            return Vec::new();
        };
        if to.is_empty() || !self.is_staff_member(to) {
            return Vec::new();
        }
        vec![
            support_line(&room, &Support::Assigned(to.to_owned())),
            support_line(&room, &Support::Joined(to.to_owned())),
        ]
    }

    /// The line that closes the guest `nick`'s room (or the room called `nick`): it folds
    /// under its channel until the guest says something again.
    #[must_use]
    pub fn close_guest(&self, nick: &str) -> Vec<Line> {
        self.support_room(nick)
            .map(|room| vec![support_line(&room, &Support::Closed)])
            .unwrap_or_default()
    }

    /// An admin's request of the Guests API about the guest `nick` of `org`: `method` on
    /// their path and then `then`. `None` unless we're an admin and `nick` is a guest.
    fn admin_request(
        &self,
        org: &str,
        nick: &str,
        method: &'static str,
        then: &str,
    ) -> Option<GuestRequest> {
        // From their room's header, the room's guest.
        let nick = zirc_proto::guest_room_of(nick).map_or(nick, |(_, guest)| guest);
        (self.admin && self.is_guest(nick)).then(|| GuestRequest {
            method,
            path: format!("{}{then}", guest_path(org, nick)),
        })
    }

    /// A support event's `TAGMSG`, from `from` to `target`: whether it was one. Staff keep
    /// all but what the guest is told; a guest keeps only that.
    pub(super) fn support_tag(&mut self, line: &zirc_line::Line, from: &str, target: &str) -> bool {
        let Some(event) = line.tag(support::TAG).and_then(Support::parse) else {
            return false;
        };
        let staff = self.is_staff();
        if event.for_guest() == staff {
            return true;
        }
        // In a guest's room, everyone keeps it there. Otherwise staff keep it in the
        // conversation with the guest; a guest, with who told them.
        let name = if is_channel(target) {
            if zirc_proto::guest_room_of(target).is_none() {
                return true;
            }
            target
        } else if !staff {
            from
        } else if self.is_guest(target) {
            target
        } else if self.is_guest(from) {
            from
        } else {
            return true;
        };
        let at = if is_channel(name) {
            let Some(at) = self.position(name) else {
                return true;
            };
            at
        } else {
            self.ensure(name, Kind::Query)
        };
        let entry = Entry {
            time: self.time_of(line),
            id: line.tag("msgid").map(str::to_owned),
            from: from.to_owned(),
            owner: None,
            kind: EntryKind::Support(event),
            history: line
                .tag("batch")
                .is_some_and(|batch| self.history.contains(batch)),
            mention: false,
            sent_from: None,
            reply_to: None,
            formatted: None,
            edited: false,
            sealed: None,
        };
        self.push(at, entry);
        true
    }

    /// Whether `entry`, newly said by a guest in the conversation called `name`, notifies:
    /// at All when it's assigned to us; otherwise as we set the channel they came from to.
    pub(super) fn guest_notifies(&self, name: &str, entry: &Entry) -> bool {
        if self.level(name) == Level::Nothing {
            return false;
        }
        let assignee = match zirc_proto::guest_room_of(name) {
            Some(_) => self.room_state(name).and_then(|state| state.assignee),
            None => self
                .conversation(name)
                .and_then(|c| state(&c.entries, name).assignee),
        };
        if assignee.is_some_and(|nick| nick.eq_ignore_ascii_case(&self.me)) {
            return true;
        }
        // A room's guest came from the public channel beside it.
        let from = zirc_proto::guest_room_of(name)
            .map(|(parent, _)| parent.to_lowercase())
            .or_else(|| self.guest_channel(name));
        let level = from.map_or(Level::default(), |channel| self.level(&channel));
        match level {
            Level::All => true,
            // The words, not the entry's mention: in a guest's own conversation, a private
            // one, every line counts as one.
            Level::Mentions => match &entry.kind {
                EntryKind::Message(text) | EntryKind::Action(text) | EntryKind::Notice(text) => {
                    crate::names::names_person(text, &self.me)
                }
                _ => false,
            },
            Level::Nothing => false,
        }
    }
}

/// The `TAGMSG` that says `event` in the conversation with the guest `nick`.
fn support_line(nick: &str, event: &Support) -> Line {
    let mut line = Line::new("TAGMSG", [nick]);
    line.tags = vec![(support::TAG.to_owned(), Some(event.to_string()))];
    line
}
