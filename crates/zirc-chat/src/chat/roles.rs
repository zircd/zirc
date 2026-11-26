//! The organisation's roles ([`zirc_proto::roles`]) as every app shows them: a nick the
//! server gives the host `role/<name>`, listed in the people list's Roles part rather than
//! among the people, with who holds it; our private conversation with it in Messages, its
//! header saying who holds it; and a vacant role's answer to what we wrote it, the box
//! saying how to fill it. How it stands is what the role's own lines last said: a seat taken
//! or left ([`SeatChange`]), or its vacant answer to us ([`Vacant`]). Every word is
//! `zirc_proto::roles`'; apps only draw.

use serde::Serialize;
use ts_rs::TS;
use zirc_proto::roles::{self, Holder, Look, Seat, SeatChange, Vacant};

use chrono::{DateTime, Utc};

use super::conversation::{Entry, EntryKind, fold};
use super::face::Face;
use super::woke::Callout;
use super::{Chat, nick_degrees};

/// What heads the people list's part for roles, under the people and over the apps.
pub const ROLES_SECTION: &str = "Roles";

/// A role in a conversation's people list, as the Roles part draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RoleMember {
    /// Its nick, `fixer`, whose conversation a click opens.
    pub nick: String,
    /// What it's called: its name.
    pub label: String,
    /// Its row's second line: `main-fix · Henrik's mbp`, or `vacant` ([`Seat::member_line`]).
    pub line: String,
    /// Whether nobody holds it, which apps draw muted.
    pub vacant: bool,
    /// Its face: its bot.
    pub face: Face,
}

impl RoleMember {
    /// It as one line, where an app has one: `fixer · main-fix · Henrik's mbp`.
    #[must_use]
    pub fn said(&self) -> String {
        format!("{} · {}", self.label, self.line)
    }
}

/// The picture and the colour, a hue in degrees, the role named `role` is drawn with
/// everywhere, in every app as on zirc.dev's Roles tab: what its admins `picked`, else the
/// gem its name tells ([`zirc_proto::agent_picture`]) and its name's colour
/// ([`nick_degrees`]).
#[must_use]
pub fn role_look(role: &str, picked: &Look) -> (String, u16) {
    (
        picked.picture_of(role).to_owned(),
        picked.hue.unwrap_or_else(|| nick_degrees(role)),
    )
}

/// What `event` says of a role's seat, when it's one of a role's own lines.
fn seat_of(event: &zirc_proto::Event) -> Option<Seat> {
    if let Some(vacant) = Vacant::of(event) {
        return Some(vacant.seat);
    }
    let change = SeatChange::of(event)?;
    let (holder, last) = if change.took {
        (Some(change.holder), None)
    } else {
        (None, Some(change.holder))
    };
    Some(Seat {
        role: change.role,
        repository: change.repository,
        holder,
        last,
        waiting: 0,
        wakes_on: Vec::new(),
        woke_today: 0,
    })
}

/// What `entry` says, when it's a role's seat taken or left.
#[must_use]
pub fn seat_change_of(entry: &Entry) -> Option<SeatChange> {
    match &entry.kind {
        EntryKind::Zirc(message) => SeatChange::of(&message.event),
        _ => None,
    }
}

/// What `entry` says, when it's a vacant role's answer to us.
#[must_use]
pub fn vacant_of(entry: &Entry) -> Option<Vacant> {
    match &entry.kind {
        EntryKind::Zirc(message) => Vacant::of(&message.event),
        _ => None,
    }
}

impl Chat {
    /// Whether `nick` is one of the organisation's roles, as its host says (`role/<name>`,
    /// which only the server sets).
    #[must_use]
    pub fn is_role(&self, nick: &str) -> bool {
        self.roles.contains(&fold(nick))
    }

    /// `nick`'s host, as a line from it or WHO says: whether it's a role.
    pub(super) fn role_hosted(&mut self, nick: &str, host: &str) {
        if zirc_proto::names::role_from_host(host).is_some() {
            self.roles.insert(fold(nick));
        } else {
            self.roles.remove(&fold(nick));
        }
    }

    /// Takes in what `event`, from `from` at `time`, says of the role `from` is, if it's
    /// one of its own lines and newer than what's known: history comes in any order.
    pub(super) fn learn_seat(
        &mut self,
        from: &str,
        time: DateTime<Utc>,
        event: &zirc_proto::Event,
    ) {
        let Some(seat) = seat_of(event) else {
            return;
        };
        let known = self
            .role_seats
            .entry(fold(from))
            .or_insert((time, seat.clone()));
        if known.0 <= time {
            *known = (time, seat);
        }
    }

    /// How the role `nick` stands, as the newest of its own lines heard says, in any
    /// conversation: `None` before it's said anything.
    #[must_use]
    pub fn role_seat(&self, nick: &str) -> Option<Seat> {
        self.role_seats
            .get(&fold(nick))
            .map(|(_, seat)| seat.clone())
    }

    /// The role `nick`'s standing, as its dot shows it: its holder's, here while its watch is
    /// open and away once it closes, as an agent's row says; away while nobody holds it (the
    /// user's choice, 26 Nov 2026: a vacant role was drawn here, as its nick always is).
    #[must_use]
    pub fn role_standing(&self, nick: &str) -> super::Standing {
        let Some(holder) = self.role_seat(nick).and_then(|seat| seat.holder) else {
            return super::Standing::Away(String::new());
        };
        match self.listening(&holder.machine, &holder.session.session) {
            Some(super::Listening::Online) => super::Standing::Here,
            Some(super::Listening::Away { .. }) => super::Standing::Away(String::new()),
            // Its watch unknown: as its machine stands.
            Some(super::Listening::Ended { .. }) | None => self.standing(&holder.machine),
        }
    }

    /// The machine `holder` works on, as people read it: `Henrik's mbp`, `your mbp`.
    fn holder_machine(&self, holder: &Holder) -> String {
        self.machine_said(&holder.machine)
    }

    /// The line under a role's name in our conversation with it: `Private to you · held by
    /// main-fix on Henrik's mbp`, `Private to you · Vacant in monorepo`.
    #[must_use]
    pub fn role_header(&self, nick: &str) -> Option<String> {
        if !self.is_role(nick) {
            return None;
        }
        let Some(seat) = self.role_seat(nick) else {
            return Some(roles::PRIVATE.to_owned());
        };
        let machine = seat
            .holder
            .as_ref()
            .map(|holder| self.holder_machine(holder))
            .unwrap_or_default();
        Some(format!("{} · {}", roles::PRIVATE, seat.status(&machine)))
    }

    /// A seat taken or left as the conversation called `name` says it: of one of our
    /// sessions, in our own conversation with the role, `your main-fix took the role`, and in
    /// a room, `your main-fix took the fixer role in monorepo`; of anyone else's, whose the
    /// session is, `main-fix (Henrik's) took the fixer role in monorepo`.
    #[must_use]
    pub fn seat_words(&self, change: &SeatChange, name: &str) -> String {
        let owner = self.whose(&change.holder.machine).map(|(owner, _)| owner);
        let ours = owner
            .as_deref()
            .is_some_and(|owner| owner.eq_ignore_ascii_case(self.me()));
        if ours {
            return if self.is_role(name) {
                change.words_yours()
            } else {
                change.words_ours()
            };
        }
        let whose = owner.map(|owner| super::possessive(&self.owner_name(&owner)));
        change.words_with(whose.as_deref())
    }

    /// The roles in the conversation called `name`, by name: the people list's Roles part,
    /// apart from its people.
    #[must_use]
    pub fn role_members(&self, name: &str) -> Vec<RoleMember> {
        let mut members: Vec<RoleMember> = self
            .people(name)
            .into_iter()
            .filter(|person| self.is_role(&person.nick))
            .map(|person| {
                let seat = self.role_seat(&person.nick);
                let holder = seat.as_ref().and_then(|seat| seat.holder.as_ref());
                let machine = holder
                    .map(|holder| self.holder_machine(holder))
                    .unwrap_or_default();
                RoleMember {
                    line: seat.as_ref().map_or_else(
                        || roles::VACANT.to_owned(),
                        |seat| seat.member_line(&machine),
                    ),
                    vacant: holder.is_none(),
                    face: self.role_face(&person.nick),
                    label: person.nick.clone(),
                    nick: person.nick,
                }
            })
            .collect();
        members.sort_by(|a, b| a.label.cmp(&b.label));
        members
    }

    /// Takes in what the server says role `nick`'s admins picked of how it looks, under
    /// [`zirc_proto::roles::LOOK_KEY`]: nothing, when it says none.
    pub(super) fn role_look_said(&mut self, nick: &str, said: Option<&str>) {
        match said.filter(|said| !said.is_empty()) {
            Some(said) => self.role_looks.insert(fold(nick), Look::of(said)),
            None => self.role_looks.remove(&fold(nick)),
        };
    }

    /// The role `nick`'s face: its gem and its colour, as its admins picked them or as its
    /// name tells ([`role_look`]), and its holder's tool's. A role is nobody's, held or
    /// vacant, so it has no owner and every app draws no owner's face on it (the user's
    /// choice, 26 Nov 2026: a vacant role's was a blank disc).
    #[must_use]
    pub fn role_face(&self, nick: &str) -> Face {
        let picked = self
            .role_looks
            .get(&fold(nick))
            .cloned()
            .unwrap_or_default();
        let (picture, hue) = role_look(&fold(nick), &picked);
        let holder = self.role_seat(nick).and_then(|seat| seat.holder);
        Face::Agent {
            tool: holder.and_then(|holder| holder.session.tool),
            hue,
            owner: String::new(),
            owner_initials: String::new(),
            owner_hue: 0,
            owner_photo: None,
            session: None,
            picture: Some(picture),
            ground: Some(hue),
        }
    }

    /// A vacant role's answer to us, as the card every app draws ([`super::RowShape::Vacant`]):
    /// who it is that nobody is, what waits, how to fill it, and who held it last.
    #[must_use]
    pub fn vacant_callout(&self, vacant: &Vacant) -> Callout {
        let machine = vacant
            .seat
            .last
            .as_ref()
            .map(|last| self.holder_machine(last));
        Callout {
            label: vacant.title(),
            spans: vec![super::woke::text(vacant.body())],
            tone: zirc_proto::woke::Tone::Person.role().to_owned(),
            button: None,
            action: None,
            command: Some(vacant.command()),
            footer: Some(vacant.footer(machine.as_deref())),
            heads: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use zirc_proto::Message;
    use zirc_proto::roles::{Holder, SeatChange};
    use zirc_proto::session::SessionCard;

    use super::*;
    use crate::chat::RowShape;
    use crate::chat::lines::tests::{at, chat, feed, utc};

    /// The fixer, its host saying it's a role, in `#api` with us.
    fn with_fixer() -> Chat {
        let mut chat = chat();
        let host = zirc_proto::names::role_host("fixer");
        feed(&mut chat, &[&format!(":fixer!u@{host} JOIN #api")]);
        chat
    }

    fn holder() -> Holder {
        Holder {
            machine: "henrik-mbp".into(),
            since: 1,
            session: SessionCard {
                session: "cc-1".into(),
                name: Some("main-fix".into()),
                title: None,
                tool: Some(zirc_proto::CLAUDE_CODE.into()),
            },
        }
    }

    /// Puts `event`, from the fixer, in the conversation `name` at `time`.
    fn from_fixer(chat: &mut Chat, name: &str, time: &str, event: &zirc_proto::Event) {
        feed(
            chat,
            &[&at(
                time,
                Some(&format!("f{time}")),
                &format!(
                    ":fixer!u@{} PRIVMSG {name} :x",
                    zirc_proto::names::role_host("fixer")
                ),
            )],
        );
        let at = chat
            .position(if name == "me" { "fixer" } else { name })
            .unwrap();
        let entry = chat.conversations[at].entries.last_mut().unwrap();
        entry.kind = EntryKind::Zirc(Message::new("fixer", event.clone()));
        let time = entry.time;
        // Put in by hand, as the event is longer than a fed line: taken in as heard.
        chat.learn_seat("fixer", time, event);
    }

    #[test]
    fn a_seat_taken_by_our_own_session_is_said_as_ours() {
        let mut chat = with_fixer();
        // Our machine, by its host.
        let host = zirc_proto::names::machine_host(chat.me());
        feed(&mut chat, &[&format!(":me-mbp!u@{host} JOIN #api")]);
        let change = SeatChange {
            role: "fixer".into(),
            repository: "monorepo".into(),
            holder: Holder {
                machine: "me-mbp".into(),
                ..holder()
            },
            took: true,
        };
        assert_eq!(
            chat.seat_words(&change, "#api"),
            "your main-fix took the fixer role in monorepo"
        );
        assert_eq!(
            chat.seat_words(&change, "fixer"),
            "your main-fix took the role"
        );
        // Anyone else's, by whose it is.
        let theirs = SeatChange {
            holder: holder(),
            ..change
        };
        assert!(chat.seat_words(&theirs, "#api").starts_with("main-fix"));
    }

    #[test]
    fn a_role_is_listed_apart_from_the_people_with_who_holds_it() {
        let mut chat = with_fixer();
        assert!(chat.is_role("fixer"));
        assert!(
            !chat
                .people_rows("#api")
                .iter()
                .any(|row| row.person.nick == "fixer")
        );
        let members = chat.role_members("#api");
        assert_eq!(members.len(), 1);
        assert_eq!(
            (members[0].line.as_str(), members[0].vacant),
            ("vacant", true)
        );
        assert!(matches!(&members[0].face, Face::Agent { picture: Some(p), .. } if p == "fixer"));
        // Vacant, its dot is away's ring, though its nick is always there.
        assert_eq!(
            chat.standing("fixer").presence(),
            super::super::Presence::Away
        );

        let took = SeatChange {
            role: "fixer".into(),
            repository: "monorepo".into(),
            holder: holder(),
            took: true,
        };
        from_fixer(&mut chat, "#api", "10:00:00", &took.event());
        let members = chat.role_members("#api");
        assert_eq!(members[0].line, "main-fix · henrik-mbp");
        assert!(!members[0].vacant);
        // Held, its holder's: away while henrik-mbp is nowhere we are, here once it is.
        assert_ne!(
            chat.standing("fixer").presence(),
            super::super::Presence::Here
        );
        feed(&mut chat, &[":henrik-mbp!u@h JOIN #api"]);
        assert_eq!(
            chat.standing("fixer").presence(),
            super::super::Presence::Here
        );
    }

    #[test]
    fn a_roles_face_is_what_its_admins_picked_once_the_server_says_it() {
        let mut chat = with_fixer();
        let said = |chat: &mut Chat, value: &[&str]| {
            let mut params = vec!["fixer", roles::LOOK_KEY, "*"];
            params.extend(value);
            let params: Vec<String> = params.into_iter().map(str::to_owned).collect();
            chat.metadata(&params);
        };
        let look = |chat: &Chat| match chat.role_face("Fixer") {
            Face::Agent {
                picture: Some(picture),
                hue,
                ground,
                ..
            } => (picture, hue, ground),
            other => panic!("{other:?}"),
        };
        let named = nick_degrees("fixer");
        assert_eq!(look(&chat), ("fixer".to_owned(), named, Some(named)));
        // A colour alone keeps its name's gem; a picture this reader can't draw, too.
        said(&mut chat, &[r#"{"picture":"hologram","hue":210}"#]);
        assert_eq!(look(&chat), ("fixer".to_owned(), 210, Some(210)));
        said(&mut chat, &[r#"{"picture":"designer","hue":30}"#]);
        assert_eq!(look(&chat), ("designer".to_owned(), 30, Some(30)));
        // The key deleted: nothing's picked any more.
        said(&mut chat, &[]);
        assert_eq!(look(&chat), ("fixer".to_owned(), named, Some(named)));
    }

    #[test]
    fn a_vacant_role_answers_with_its_card_and_the_command_to_copy() {
        let mut chat = with_fixer();
        let vacant = Vacant {
            seat: Seat {
                role: "fixer".into(),
                repository: "monorepo".into(),
                holder: None,
                last: Some(holder()),
                waiting: 1,
                wakes_on: Vec::new(),
                woke_today: 0,
            },
            reply_to: None,
        };
        from_fixer(&mut chat, "me", "10:00:00", &vacant.event());
        assert_eq!(
            chat.header_line("fixer").unwrap().text,
            "Private to you · Vacant in monorepo"
        );
        let lines = chat.lines("fixer", utc(), false);
        let line = lines
            .iter()
            .find(|line| line.shape == RowShape::Vacant)
            .expect("the vacant card");
        let callout = line.callout.as_ref().unwrap();
        assert_eq!(callout.label, "Nobody holds fixer in monorepo");
        // The command in a box of its own, its words around it, not in them.
        assert_eq!(callout.command.as_deref(), Some("zirc role take fixer"));
        assert!(
            callout
                .spans
                .iter()
                .all(|span| !span.text.contains("zirc role"))
        );
        assert!(callout.footer.as_deref().is_some_and(|footer| {
            footer.starts_with("Last held by main-fix (Claude Code) on henrik-mbp")
        }));
        assert!(callout.said().contains("zirc role take fixer"));
    }

    #[test]
    fn a_tools_report_is_the_roles_card_in_the_room_naming_the_tool() {
        use zirc_proto::inbound::{Tool, card_event};
        use zirc_proto::role_report::{OpenItem, RoleReport, Subject};
        let mut chat = with_fixer();
        let report = RoleReport {
            tool: Some(Tool::Sentry),
            summary: "TypeError in web: Cannot read properties of undefined".into(),
            link: Some("https://zirc.sentry.io/issues/7/".into()),
            ..RoleReport::new("fixer", Subject::Error, "sentry:7")
        };
        let item = OpenItem::opened(report, 0);
        from_fixer(&mut chat, "#api", "10:00:00", &card_event(&item, true));
        let lines = chat.lines("#api", utc(), false);
        let line = lines.last().unwrap();
        assert_eq!(line.shape, RowShape::System);
        assert_eq!(line.from, "fixer");
        // The role's tag after its name, as an app's after its.
        assert_eq!(
            line.speaker
                .as_ref()
                .and_then(|speaker| speaker.tag.as_deref()),
            Some(crate::chat::ROLE_TAG)
        );
        let said: Vec<&str> = line
            .rich
            .iter()
            .filter_map(|block| match block {
                crate::chat::RichBlock::Paragraph { spans } => Some(spans),
                _ => None,
            })
            .flat_map(|spans| spans.iter().map(|span| span.text.as_str()))
            .collect();
        assert_eq!(
            said,
            [
                "TypeError in web: Cannot read properties of undefined",
                "Open in Sentry",
                "Sentry · woke fixer · one open item",
            ]
        );
    }

    #[test]
    fn a_roles_wakes_show_in_our_conversation_with_it_saying_who_woke_as_it() {
        use zirc_proto::woke::Woke;
        let mut chat = with_fixer();
        // Our conversation with the fixer, as when we wrote it.
        from_fixer(
            &mut chat,
            "me",
            "09:00:00",
            &zirc_proto::Event::Text {
                body: "on it".into(),
            },
        );
        // The holder's machine says its wake in the room, as its zircd does.
        let host = zirc_proto::names::machine_host("henrik");
        feed(&mut chat, &[&format!(":henrik-mbp!u@{host} JOIN #api")]);
        let woke = Woke {
            role: Some("fixer".into()),
            ..Woke::webhook("backup job nightly-db finished with warnings")
        };
        feed(
            &mut chat,
            &[&at(
                "10:00:00",
                Some("w1"),
                &format!(":henrik-mbp!u@{host} PRIVMSG #api :x"),
            )],
        );
        // Put in by hand, as the event is longer than a fed line, as zircd's multiline is.
        let at = chat.position("#api").unwrap();
        chat.conversations[at].entries.last_mut().unwrap().kind =
            EntryKind::Zirc(Message::new("cc-1", woke.event()));
        let lines = chat.lines("fixer", utc(), false);
        let wakes: Vec<&crate::chat::lines::Line> = lines
            .iter()
            .filter(|line| line.shape == RowShape::Woke)
            .collect();
        assert_eq!(wakes.len(), 1);
        assert_eq!(
            wakes[0].text,
            "henrik's cc-1 woke as the fixer: A webhook from an unknown tool, as it was sent: “backup job nightly-db finished with warnings”"
        );
        // Below what was said before it, and nothing unread for it.
        assert!(
            lines
                .last()
                .is_some_and(|line| line.shape == RowShape::Woke)
        );
        let unread = chat.conversation("fixer").unwrap().unread;
        // The fixer's own line before it counts once; the wake adds nothing.
        assert_eq!(unread, 1);
    }

    #[test]
    fn a_line_ops_hides_is_not_what_its_row_previews() {
        let mut chat = with_fixer();
        feed(
            &mut chat,
            &[
                ":me!u@h JOIN #ops",
                &format!(
                    ":fixer!u@{} JOIN #ops",
                    zirc_proto::names::role_host("fixer")
                ),
            ],
        );
        // What the role says at info: #ops shows what needs someone (warn) until its reader
        // picks otherwise, as a report does now (a4e303b), so its row previews nothing of it.
        let said = zirc_proto::Event::Text {
            body: "on it".into(),
        };
        from_fixer(&mut chat, "#ops", "10:00:00", &said);
        assert!(chat.lines("#ops", utc(), false).is_empty());
        assert_eq!(chat.preview("#ops"), None);
        chat.set_threshold("#ops", zirc_proto::Level::Info);
        assert!(chat.preview("#ops").is_some());
    }
}
