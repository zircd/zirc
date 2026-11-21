//! The client's state and how it changes: [`App::handle`] takes each line from the server,
//! [`App::key`] each key pressed and [`App::pointer`] each click or turn of the wheel, and
//! they say which lines to send back. The chat itself
//! (conversations, members, history, unread counts and mentions) is zirc-core's
//! [`Chat`], which zirc's other apps share; this adds what a terminal needs on top: typed
//! commands, the input line, and scrolling. Nothing here touches the network or the
//! terminal, so all of it is tested line by line.

use std::cell::RefCell;
use std::collections::HashMap;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use zirc_core::chat::{
    Chat, Conversation, Details, Entry, Face, GuestRequest, Here, Kind, ListKind, ListOptions,
    ListRow, Options, Quote, Ran, RecentSearches, SearchScreen, Shelf, complete_mention, copy_text,
    entry_text, first_link, fold, is_channel,
};
use zirc_irc::Line;

use crate::hyperlink::Links;
use zirc_proto::telemetry::{Event, FeatureUsed};

pub use zirc_core::chat::Link;

use crate::command::{self, Command};
use crate::find::{Searched, Searching};
use crate::input::Input;
use crate::jump::{Jumped, Jumping};
use crate::picker::{self, Picked, Picker};
use crate::rows::{self, Row, Zone};
use crate::scrollback::{Anchor, Cache, Laid, View};
use zirc_core::agents::{self, Queries};
use zirc_core::command::said;
use zirc_core::compose::{self, Completing};
use zirc_core::connection::Status;
use zirc_core::emoji::{self, Recent};
use zirc_core::ipc::{Request, Response};
use zirc_core::keys;
use zirc_core::orgs::Unread;
use zirc_theme::Theme;

/// How many messages of history to ask for when joining a conversation.
pub const HISTORY_LIMIT: usize = 200;

/// How many rows a turn of the mouse wheel scrolls.
const WHEEL_ROWS: usize = 3;

/// What's at a place on the screen, for the mouse: see [`crate::ui::target_at`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A conversation in the list down the left, by where it is in [`App::conversations`].
    Conversation(usize),
    /// An agent session we haven't talked to yet, below them ([`App::unopened_agents`]).
    Agent(String),
    /// Someone in the member list.
    Member(String),
    /// A message in the scrollback, by id.
    Message(String),
    /// A reaction under the message `id` in the scrollback.
    Reaction {
        /// The message.
        id: String,
        /// The reaction's emoji.
        emoji: String,
    },
    /// A section's heading, which folds the section away or opens it.
    Shelf(Shelf),
    /// A half of the sidebar's `Channels | Messages` switch, which shows it.
    Tab(zirc_core::chat::SidebarSection),
    /// A guest's room we aren't in, under its channel: opening it joins it.
    Open(String),
    /// The row a channel's guests' rooms fold under when there are many, by the channel.
    GuestRooms(String),
    /// The scrollback, anywhere else.
    Scrollback,
}

/// A message about to be trimmed from the server's history, once the person says so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trimming {
    /// The conversation it's in.
    pub target: String,
    /// Its id.
    pub id: String,
    /// What it says, as the scrollback shows it, to be sure it's the one.
    pub text: String,
    /// Why, if the person said.
    pub reason: Option<String>,
}

/// What to do after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send these lines, possibly none, and carry on.
    Send(Vec<Line>),
    /// Send these lines and stop.
    Quit(Vec<Line>),
    /// Ask this machine's zircd this, and hand its answer to [`App::zircd_answered`].
    Zircd(Request),
    /// Show the organisation named, or the next one, on a client signed in to several.
    Org(Option<String>),
    /// Ask this machine's zircd what every session is doing, and hand it to
    /// [`App::sessions_answered`].
    Sessions,
    /// Make this request of zirc.dev's Guests API as an admin, and hand
    /// [`App::guests_answered`] `done` once it's done, or why not.
    Guests {
        /// The request.
        request: GuestRequest,
        /// What to say once it's done.
        done: String,
    },
    /// Open this page in the browser.
    Open(String),
    /// Press a card's button through zirc.dev, and hand [`App::press_answered`] what came
    /// of it.
    Press(zirc_proto::card::Press),
}

/// A card's button about to be pressed, once the person says so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pressing {
    /// The press.
    pub press: zirc_proto::card::Press,
    /// What to ask: the button's own question, or `press Retry?`.
    pub ask: String,
}

/// What an admin does with a guest through zirc.dev.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Admin {
    /// Refuse them in every public channel.
    Ban,
    /// Let them back.
    Unban,
    /// Invite them to join the organisation.
    Invite,
}

/// Where an organisation's zirc.dev is, for what its admins do there rather than over IRC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// The web app's address.
    pub web: String,
    /// The organisation's short name there.
    pub slug: String,
    /// zirc's directory, where the sign-in is kept, so a new nickname can be put in it.
    pub home: Option<std::path::PathBuf>,
}

/// The sessions panel, while it's open: what every session is doing, as zircd says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionsPanel {
    /// The sessions, most lately heard from first, once zircd's answered, or why it
    /// couldn't say.
    pub listed: Option<Result<Vec<zirc_core::sessions::Session>, String>>,
    /// When it answered, in seconds since the Unix epoch, for how long ago each was heard.
    pub at: u64,
}

/// The emoji Tab offers for a `:shortcode`, as the other apps' lists do, and the one written.
#[derive(Debug)]
struct Tabbed {
    /// The input before the first Tab, and its cursor.
    typed: (String, usize),
    /// The emoji offered.
    offered: Vec<&'static str>,
    /// The one written.
    at: usize,
    /// The input once it's written, and its cursor: Tab again moves on only from there.
    wrote: (String, usize),
}

impl Tabbed {
    /// The first emoji for `query`, the `:shortcode` typed in `text` at `at`, written; none
    /// when nothing matches. Tab goes through as many as the other apps' lists show.
    fn new(query: &str, text: &str, at: usize) -> Option<Self> {
        let offered: Vec<&'static str> = emoji::suggest(query, picker::NEWEST, emoji::OFFERED)
            .into_iter()
            .map(|found| found.emoji)
            .collect();
        let wrote = emoji::complete_shortcode(text, at, offered.first()?);
        Some(Self {
            typed: (text.to_owned(), at),
            offered,
            at: 0,
            wrote,
        })
    }

    /// The next emoji written instead, when the input at `at` is still `text`, what Tab
    /// wrote last.
    fn next(mut self, text: &str, at: usize) -> Option<Self> {
        if (text, at) != (self.wrote.0.as_str(), self.wrote.1) {
            return None;
        }
        self.at = (self.at + 1) % self.offered.len();
        self.wrote = emoji::complete_shortcode(&self.typed.0, self.typed.1, self.offered[self.at]);
        Some(self)
    }
}

/// Everything the client shows.
#[derive(Debug)]
pub struct App {
    /// The chat: its conversations and what's been said in them.
    pub chat: Chat,
    /// What's being typed.
    pub input: Input,
    /// Which time zone to show times in.
    pub zone: Zone,
    /// How the terminal shows links: clickable labels, or as written.
    pub links: Links,
    /// The conversations, folded, showing only what people say (`/people`).
    people_only: std::collections::BTreeSet<String>,
    /// The editor code links open in, as the person chose it in `tui-prefs.toml`, by its id
    /// ([`zirc_core::editor::editor`]): the first installed when unset.
    editor: Option<String>,
    /// A card's button about to be pressed, once the person says so.
    pressing: Option<Pressing>,
    /// What came of our presses, by the version of the card pressed.
    pressed: Presses,
    /// The size of the scrollback, in columns and rows, as last drawn.
    pub view: (u16, u16),
    /// zirc's colours, dark or light.
    pub theme: &'static Theme,
    /// Where the connection is, as its supervisor says.
    pub connection: Status,
    /// Where each conversation scrolled up from the newest is read from, by folded name.
    scrolled: HashMap<String, Anchor>,
    /// The rows of the scrollback's lines, as last laid out.
    laid: RefCell<Cache>,
    /// Counts changes that may change how any item is laid out, such as a name, rather
    /// than what an item is: each one lays out afresh what's shown.
    changes: u64,
    /// Whose profile is shown over the scrollback, by nick.
    profile: Option<String>,
    /// The agent session whose card is shown over the scrollback: its machine and its id.
    agent: Option<(String, String)>,
    /// The message picked to react to: its conversation, folded, and its id.
    picked: Option<(String, String)>,
    /// The reaction picker, when it's open.
    picker: Option<Picker>,
    /// Jump to, when it's open (Alt-K).
    jumping: Option<Jumping>,
    /// Search, when it's open (Alt-F).
    searching: Option<Searching>,
    /// The searches made lately on this server, newest first ([`RecentSearches`]).
    recent_searches: RecentSearches,
    /// The picked message's details, shown over the scrollback (`i`), and how many of its
    /// lines are scrolled past.
    details: Option<(Details, u16)>,
    /// The emoji reacted with lately, which the picker offers first.
    pub recent: Recent,
    /// The message about to be trimmed, waiting for the person to confirm it.
    trimming: Option<Trimming>,
    /// The channel about to be left, and why, waiting for the person to confirm it.
    leaving: Option<(String, Option<String>)>,
    /// Browse channels, while it's shown over the scrollback (`/browse`).
    browsing: Option<BrowsePanel>,
    /// The message the next line sent answers: its conversation, folded, and its id.
    replying: Option<(String, String)>,
    /// Our message the next line sent corrects, by the server's id, in the conversation
    /// (folded name) it's in, with what it said when it was picked: `e` on a message starts
    /// it.
    editing: Option<(String, String, String)>,
    /// What to put on the clipboard, until it's taken.
    clipboard: Option<String>,
    /// What the last key did, as copying or pinning, to say so until the next.
    flash: Option<String>,
    /// What was typed, when the last key sent it, so a send that fails can give it back.
    sent_typed: Option<String>,
    /// On a client signed in to several organisations, each of them with what's unread
    /// there, and which this app is, as whoever holds them says before each draw. Empty
    /// with one: then nothing is said about organisations.
    pub orgs: Vec<(String, Unread)>,
    /// Which of [`App::orgs`] this is.
    pub org: Option<String>,
    /// Where the organisation's zirc.dev is, if it has one.
    pub site: Option<Site>,
    /// The sessions panel, while it's open.
    sessions: Option<SessionsPanel>,
    /// While another organisation is shown, the conversation this one showed, to show
    /// again when it's back: meanwhile it shows the server's, so what's said doesn't count
    /// as read.
    hidden: Option<String>,
    /// Whether what [`App::prefs`] holds changed since it was last taken.
    prefs_changed: bool,
    /// The emoji Tab wrote for a `:shortcode`, which Tab again moves on from.
    tabbed: Option<Tabbed>,
    /// Whether others are told we're typing.
    send_typing: bool,
}

/// What came of our presses, by the version of the card pressed
/// ([`zirc_core::chat::CardButtons::key`]): a card posted again is a new version, which
/// shows nothing of them.
#[derive(Debug, Default)]
struct Presses(HashMap<String, String>);

impl rows::Pressed for Presses {
    fn pressed(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
}

/// A row of the conversation list down the left, as [`App::sidebar`] lays it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Side {
    /// The organisation's name and how many people are in it, over everything, as
    /// zirc-chat's [`zirc_core::chat::Chat::org_header`] says.
    Org(zirc_core::chat::OrgHeader),
    /// The `Channels | Messages` switch under it, as every app shows one half of the list at
    /// a time ([`zirc_core::chat::Chat::tabs`]): a click on a half shows it.
    Tabs(Vec<zirc_core::chat::SidebarTab>),
    /// A section's heading: a click folds the section away, or opens it.
    Heading {
        /// What it says, `Channels`.
        label: &'static str,
        /// Which section it heads.
        shelf: Shelf,
        /// Whether the section is open; folded, it keeps only what's new or shown.
        open: bool,
    },
    /// A row of a section, `depth` steps in.
    Row {
        /// What it shows.
        label: String,
        /// What a click on it does: shows its conversation, or opens an agent session's.
        /// None for a row that only gathers the rows under it.
        target: Option<Target>,
        /// How far in it's nested: a branch room under its channel, a machine under its
        /// person, an agent session under its machine.
        depth: usize,
        /// Whether it shows the unread pip at the edge: zirc-chat's `ListRow::pip`.
        pip: bool,
        /// For an agent session, its tool's mark and its hue in degrees, drawn in front.
        agent: Option<(&'static str, u16)>,
        /// Whether its conversation is muted: drawn muted, never bold.
        muted: bool,
        /// For a person, what was said last and when, as zirc-chat's list says them:
        /// `bob: yeah exactly`, `5m`.
        preview: Option<(String, String)>,
        /// Whether `preview` is zirc-chat's `No messages yet`, with no time: drawn faint.
        placeholder: bool,
        /// For an agent session at work, what it's taking on, after its name in place of
        /// what was said.
        note: Option<String>,
        /// For a person, their presence, drawn as a mark before their name.
        presence: Option<zirc_core::chat::Presence>,
        /// Whether anything in it, or for a channel under it, is unread: bold, with a pip.
        unread: bool,
        /// What zirc-chat counts for it, in a pill: mentions in a channel, everything in a
        /// direct message.
        badge: u32,
        /// For a channel with guests' rooms, its name and whether they're open under it: a
        /// mark at the row's end, `▸` or `▾`, that a click folds or opens.
        fold: Option<(String, bool)>,
    },
}

/// What the client keeps for the person between runs: see [`crate::prefs`].
pub use crate::prefs::Prefs;

impl App {
    /// A client for `server` that will join `joins` besides the channels the server lists.
    #[must_use]
    pub fn new(nick: &str, server: &str, joins: Vec<String>) -> Self {
        let options = Options {
            history: HISTORY_LIMIT,
            batch_prefix: "tui".to_owned(),
            client: Some("tui".to_owned()),
            ..Options::default()
        };
        Self {
            chat: Chat::new(nick, server, joins, options),
            input: Input::default(),
            zone: Zone::Local,
            links: Links::Plain,
            people_only: std::collections::BTreeSet::new(),
            editor: None,
            pressing: None,
            pressed: Presses::default(),
            view: (80, 20),
            theme: Theme::dark(),
            connection: Status::Connecting {
                attempt: 1,
                was_up: false,
            },
            scrolled: HashMap::new(),
            laid: RefCell::default(),
            changes: 0,
            profile: None,
            agent: None,
            picked: None,
            picker: None,
            jumping: None,
            searching: None,
            recent_searches: RecentSearches::default(),
            details: None,
            recent: Recent::default(),
            trimming: None,
            leaving: None,
            browsing: None,
            replying: None,
            editing: None,
            clipboard: None,
            orgs: Vec::new(),
            sessions: None,
            org: None,
            site: None,
            hidden: None,
            flash: None,
            sent_typed: None,
            prefs_changed: false,
            tabbed: None,
            send_typing: true,
        }
    }

    /// Our nick.
    #[must_use]
    pub fn nick(&self) -> &str {
        self.chat.me()
    }

    /// The server, as shown.
    #[must_use]
    pub fn server(&self) -> &str {
        self.chat.server()
    }

    /// Whether we're connected.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.chat.is_connected()
    }

    /// The conversations: the server's first, then channels, then private ones.
    #[must_use]
    pub fn conversations(&self) -> &[Conversation] {
        self.chat.conversations()
    }

    /// The conversations in the list down the left, with where each is in
    /// [`App::conversations`]: all but the server's, whose words are in the status line, in
    /// the order the list down the left shows them ([`App::sidebar`]).
    pub fn listed(&self) -> impl Iterator<Item = (usize, &Conversation)> {
        let conversations = self.conversations();
        // Both halves of the switch, one after the other: going to the next by key reaches
        // every conversation, and its half shows once it's shown.
        self.sides(None)
            .into_iter()
            .filter_map(|row| match row {
                Side::Row {
                    target: Some(Target::Conversation(at)),
                    ..
                } => Some(at),
                _ => None,
            })
            .map(move |at| (at, &conversations[at]))
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// The list down the left, as zirc-chat lists it for every app
    /// ([`Chat::sidebar_list`]), under the organisation's name and the `Channels | Messages`
    /// switch, for the half shown: what's starred in it, then the channels A–Z, each
    /// repository's with its rooms under it; or the direct
    /// messages (people, groups, agent sessions and our machines' threads) newest first, our
    /// machines online and, for staff, the conversations with guests, the closed ones folded.
    #[must_use]
    pub fn sidebar(&self) -> Vec<Side> {
        let mut rows = Vec::new();
        let org = self.chat.org_header();
        if !org.name.is_empty() {
            rows.push(Side::Org(org));
        }
        rows.push(Side::Tabs(self.chat.tabs()));
        rows.extend(self.sides(Some(self.chat.tab())));
        rows
    }

    /// The list's rows for `tab`, or for both halves.
    pub(crate) fn sides(&self, tab: Option<zirc_core::chat::SidebarSection>) -> Vec<Side> {
        self.chat
            .sidebar_list(&ListOptions { tab })
            .into_iter()
            .map(|row| self.side_of(row))
            .collect()
    }

    /// The row zirc-chat lists as `row`, as the list down the left draws it.
    fn side_of(&self, row: ListRow) -> Side {
        let target = match row.kind {
            ListKind::Heading => {
                return Side::Heading {
                    label: row.shelf.label(),
                    shelf: row.shelf,
                    open: row.open != Some(false),
                };
            }
            ListKind::Conversation | ListKind::Room | ListKind::GuestRoom => {
                self.side_target(&row.name)
            }
        };
        // An agent session: its tool's mark in front, in its colour.
        let agent = match row.faces.first() {
            Some(Face::Agent { tool, hue, .. }) if row.direct_kind.is_some() => {
                Some((zirc_core::chat::tool_mark(tool.as_deref()), *hue))
            }
            _ => None,
        };
        // A conversation with a guest assigned to us: a dot after its name.
        let guest = row.guest && row.kind == ListKind::Conversation;
        let thread = row.kind == ListKind::Room;
        let label = if guest && row.mine {
            format!("{} •", row.title)
        } else if thread {
            // A thread under its channel: a branch mark from it.
            format!("└ {}", row.title)
        } else {
            row.title
        };
        // After a name, a thread's under its channel too, what was said last in it and when,
        // as chat apps list them; for an agent at work, what it's taking on.
        let private = row.direct_kind.is_some();
        // Nothing said in it yet: zirc-chat's `No messages yet`, with no time.
        let when = row.when.or_else(|| row.placeholder.then(String::new));
        let preview = row.preview.zip(when);
        let note = row.note.filter(|_| private);
        Side::Row {
            label,
            target,
            depth: usize::from(row.depth),
            pip: row.pip,
            agent,
            muted: row.muted,
            preview,
            placeholder: row.placeholder,
            note,
            presence: row.presence.filter(|_| private),
            unread: row.unread,
            badge: row.badge,
            fold: row
                .open
                .filter(|_| row.kind == ListKind::Conversation)
                .map(|open| (row.name.clone(), open)),
        }
    }

    /// Carries out what an admin does with a guest through zirc.dev, when
    /// `command` is one of those in the conversation `here`; `None` for anything else.
    fn guest_admin(&mut self, command: &Command, here: &str) -> Option<Action> {
        let shown = self.chat.guest_of(here);
        let (what, nick) = match command {
            Command::Ban => (Admin::Ban, shown?),
            Command::Unban(nick) => (Admin::Unban, nick.clone().or(shown)?),
            // Alone, `/invite` invites the guest shown; with a nick, a guest or someone to a group.
            Command::Invite(nicks) => match nicks.as_slice() {
                [] if shown.is_none() => {
                    self.note("/invite needs a nick".into());
                    return Some(Action::Send(Vec::new()));
                }
                [] => (Admin::Invite, shown?),
                [nick] if self.chat.is_guest(nick) => (Admin::Invite, nick.clone()),
                // A guest is only ever in their own conversation with staff, never a group.
                nicks if nicks.iter().any(|nick| self.chat.is_guest(nick)) => {
                    self.note("a guest can't join a group: /invite <guest> alone invites them to the organisation".into());
                    return Some(Action::Send(Vec::new()));
                }
                _ => return None,
            },
            _ => return None,
        };
        Some(self.admin(what, &nick))
    }

    /// Asks zirc.dev to do `what` with the guest `nick`, if we're an admin there.
    fn admin(&mut self, what: Admin, nick: &str) -> Action {
        let verb = match what {
            Admin::Ban => "ban",
            Admin::Unban => "unban",
            Admin::Invite => "invite",
        };
        let refused = if !self.chat.is_admin() {
            Some(format!("only an admin can {verb} a guest"))
        } else if self.site.is_none() {
            Some(format!(
                "this server isn't zirc.dev's: {verb} guests where it's run"
            ))
        } else {
            None
        };
        let slug = self.site.as_ref().map(|site| site.slug.clone());
        let request = slug.and_then(|slug| match what {
            Admin::Ban => self.chat.ban_guest(&slug, nick),
            Admin::Unban => self.chat.unban_guest(&slug, nick),
            Admin::Invite => self.chat.invite_guest(&slug, nick),
        });
        let Some(request) = request.filter(|_| refused.is_none()) else {
            self.note(refused.unwrap_or_else(|| format!("{nick} isn't a guest here")));
            return Action::Send(Vec::new());
        };
        let name = self.chat.guest_name(nick);
        let done = match what {
            Admin::Ban => format!("{name} is banned from every public channel"),
            Admin::Unban => format!("{name} may come back to the public channels"),
            Admin::Invite => format!("{name} is invited to join the organisation"),
        };
        Action::Guests { request, done }
    }

    /// Shows only what people say in the conversation shown, and whatever mentions us, or
    /// everything again: remembered for it, as the other apps do.
    fn toggle_people(&mut self) -> Vec<Line> {
        let name = fold(&self.current().name);
        if !self.people_only.remove(&name) {
            self.people_only.insert(name);
        }
        self.prefs_changed = true;
        Vec::new()
    }

    /// `/show`: with `choice`, an entry of the conversation's "Show" menu by its label or
    /// level, makes it show that; with none, says what it can show, its choice marked.
    fn show(&mut self, choice: Option<&str>) -> Vec<Line> {
        let name = self.current().name.clone();
        let choices = self.chat.show_choices(&name);
        let Some(choice) = choice else {
            let listed: Vec<String> = choices
                .iter()
                .map(|c| {
                    let label = c.label.to_lowercase();
                    if c.checked {
                        format!("[{label}]")
                    } else {
                        label
                    }
                })
                .collect();
            self.note(format!(
                "{} in {name}: {}",
                zirc_core::chat::SHOW_LABEL,
                listed.join(" · ")
            ));
            return Vec::new();
        };
        let picked = choices.iter().find(|c| {
            c.label.eq_ignore_ascii_case(choice) || c.level.as_str().eq_ignore_ascii_case(choice)
        });
        let Some(picked) = picked else {
            return self.refuse("/show errors, warnings, info or everything");
        };
        self.chat.set_threshold(&name, picked.level);
        self.prefs_changed = true;
        Vec::new()
    }

    /// `/editor`: with `choice`, one of "Open code in"'s by its id or label, has code links
    /// open there; with none, says the choices, the one chosen marked.
    fn choose_editor(&mut self, choice: Option<&str>) -> Vec<Line> {
        let options = zirc_core::editor::options(self.editor.as_deref());
        let Some(choice) = choice else {
            let listed: Vec<String> = options
                .iter()
                .map(|o| {
                    if o.chosen {
                        format!("[{}] {}", o.id, o.label)
                    } else {
                        format!("{} {}", o.id, o.label)
                    }
                })
                .collect();
            self.note(format!(
                "{}: {}",
                zirc_core::chat::OPEN_CODE_IN,
                listed.join(" · ")
            ));
            return Vec::new();
        };
        // Any editor by its id, installed here or not yet.
        let picked = options
            .iter()
            .find(|o| o.id.eq_ignore_ascii_case(choice) || o.label.eq_ignore_ascii_case(choice))
            .map(|o| o.id.clone())
            .or_else(|| zirc_core::chat::Editor::from_id(choice).map(|e| e.id().to_owned()));
        let Some(picked) = picked else {
            let ids: Vec<&str> = options.iter().map(|o| o.id.as_str()).collect();
            return self.refuse(&format!("/editor {}", ids.join(", ")));
        };
        self.editor = zirc_core::editor::kept(&picked);
        self.chat
            .set_editor(zirc_core::editor::editor(self.editor.as_deref()));
        self.prefs_changed = true;
        Vec::new()
    }

    /// What the conversation shown leaves out, if anything, as the status line says it:
    /// `warnings and up` when it shows from warnings.
    #[must_use]
    pub fn showing(&self) -> Option<String> {
        let name = &self.current().name;
        let level = self.chat.threshold(name);
        (level != zirc_core::chat::default_threshold(name)).then(|| {
            let choices = self.chat.show_choices(name);
            let label = choices
                .iter()
                .find(|c| c.checked)
                .map_or("", |c| c.label.as_str())
                .to_lowercase();
            if level == zirc_proto::Level::Debug {
                label
            } else {
                format!("{label} and up")
            }
        })
    }

    /// Whether the conversation shown shows only what people say (`/people`).
    #[must_use]
    pub fn people_only(&self) -> bool {
        self.people_only.contains(&fold(&self.current().name))
    }

    /// Says `why` a command can't be carried out here, and sends nothing.
    fn refuse(&mut self, why: &str) -> Vec<Line> {
        self.note(why.to_owned());
        Vec::new()
    }

    /// Opens our account's page on zirc.dev, where our name, picture and nickname are set.
    fn profile_page(&mut self) -> Action {
        let Some(site) = &self.site else {
            self.note("this server isn't zirc.dev's: there's no account page to open".into());
            return Action::Send(Vec::new());
        };
        Action::Open(format!(
            "{}{}",
            site.web.trim_end_matches('/'),
            zirc_proto::profile::ACCOUNT_PAGE_PATH
        ))
    }

    /// Leaving a channel, waiting on the person: y leaves, Ctrl-C quits, and anything else
    /// keeps it. `None` when no leaving is waiting.
    fn leaving_key(&mut self, key: KeyEvent) -> Option<Action> {
        let (channel, reason) = self.leaving.take()?;
        Some(match key.code {
            KeyCode::Char('y' | 'Y') => Action::Send(self.chat.part(&channel, reason.as_deref())),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Action::Quit(vec![quit(None)])
            }
            _ => Action::Send(Vec::new()),
        })
    }

    /// Pressing a card's button, waiting on the person: y presses, anything else doesn't.
    /// `None` when no button is waiting.
    fn pressing_key(&mut self, key: KeyEvent) -> Option<Action> {
        let pressing = self.pressing.take()?;
        Some(if matches!(key.code, KeyCode::Char('y' | 'Y')) {
            Action::Press(pressing.press)
        } else {
            Action::Send(Vec::new())
        })
    }

    /// The button pressing, waiting on the person's y or n, if one is.
    #[must_use]
    pub fn pressing(&self) -> Option<&Pressing> {
        self.pressing.as_ref()
    }

    /// Presses the button `name`, its id or its label, on the card picked with Shift-↑, or the
    /// latest card in the conversation shown: asking first only when the card asks, as in
    /// every app (the user's choice, 20 Nov 2026).
    fn press(&mut self, name: &str) -> Action {
        let picked = self.picked().map(str::to_owned);
        let channel = self.current().name.clone();
        // The cards here with buttons, those zirc-chat shows us ([`Chat::shows_action`]).
        let cards: Vec<(zirc_core::chat::CardButtons, Option<String>, String)> = {
            let laid = self.laid(usize::from(self.view.0));
            laid.lines()
                .iter()
                .filter_map(|line| {
                    let buttons = line.system.as_ref()?.buttons.clone()?;
                    Some((buttons, line.id.clone(), line.text.clone()))
                })
                .collect()
        };
        let card = cards
            .iter()
            .find(|(_, id, _)| id.is_some() && *id == picked)
            .or_else(|| cards.last());
        let Some((buttons, _, title)) = card else {
            return Action::Send(self.refuse("there's no card here to press"));
        };
        let Some(action) = buttons.actions.iter().find(|action| {
            action.id.eq_ignore_ascii_case(name) || action.label.eq_ignore_ascii_case(name)
        }) else {
            let title = title.lines().next().unwrap_or_default();
            return Action::Send(self.refuse(&format!("\"{title}\" has no button {name} for you")));
        };
        let press = zirc_proto::card::Press {
            channel,
            sender: buttons.sender.clone(),
            card: buttons.card.clone(),
            action: action.id.clone(),
        };
        match action.confirm.clone() {
            Some(ask) => {
                self.pressing = Some(Pressing { press, ask });
                Action::Send(Vec::new())
            }
            None => Action::Press(press),
        }
    }

    /// What came of pressing a card's button, `press`: what the integration said, or why it
    /// couldn't. It shows on the card until the card changes.
    pub fn press_answered(
        &mut self,
        press: &zirc_proto::card::Press,
        answer: Result<Option<String>, String>,
    ) {
        let said = match answer {
            Ok(message) => message.unwrap_or_else(|| zirc_core::chat::DONE.to_owned()),
            Err(why) => zirc_core::chat::couldnt_label(&why),
        };
        // On the card as it is now: one posted again since shows nothing of it.
        let key = self
            .chat
            .lines(&press.channel, self.zone.offset(), false)
            .iter()
            .filter_map(|line| line.system.as_ref()?.buttons.as_ref())
            .find(|buttons| {
                buttons.card == press.card && fold(&buttons.sender) == fold(&press.sender)
            })
            .map(|buttons| buttons.key.clone());
        if let Some(key) = key {
            self.pressed.0.insert(key, said);
        }
        self.changed();
    }

    /// The account was renamed to `handle` on another device or at zirc.dev: what to say of
    /// it, as the connection follows.
    pub fn renamed_elsewhere(&mut self, handle: &str) {
        self.note(format!(
            "Your nickname was changed to {handle} elsewhere: reconnecting as it."
        ));
    }

    /// What zirc.dev answered a request of its Guests API: what it did, or why it didn't.
    pub fn guests_answered(&mut self, answer: Result<String, String>) {
        self.note(answer.unwrap_or_else(|why| format!("zirc.dev: {why}")));
    }

    /// What a click on the list's row for the conversation called `name` does: shows it,
    /// or opens an agent session's not yet talked in.
    fn side_target(&self, name: &str) -> Option<Target> {
        match self.chat.position(name) {
            Some(at) => Some(Target::Conversation(at)),
            None if zirc_core::chat::is_guest_room(name) => Some(Target::Open(name.to_owned())),
            None => self
                .chat
                .agent(name)
                .map(|_| Target::Agent(name.to_owned())),
        }
    }

    /// Where the conversation shown is in [`App::conversations`].
    #[must_use]
    pub fn selected(&self) -> usize {
        self.chat.selected_index()
    }

    /// The conversation shown.
    #[must_use]
    pub fn current(&self) -> &Conversation {
        self.chat.selected()
    }

    /// Whether the conversation shown is scrolled up from the newest.
    #[must_use]
    pub fn scrolled_back(&self) -> bool {
        self.scrolled.contains_key(&fold(&self.current().name))
    }

    /// How many rows of the conversation shown are below the scrollback, scrolled up from
    /// the newest. It lays out every one of them: for tests.
    #[must_use]
    pub fn scroll(&self) -> usize {
        let laid = self.laid(usize::from(self.view.0));
        let view = self.view_of(&laid, usize::from(self.view.1));
        view.top.filter(|_| !view.at_end).map_or(0, |top| {
            laid.to_end(top).saturating_sub(usize::from(self.view.1))
        })
    }

    /// How many lines have been laid out afresh rather than found laid out already: for
    /// tests.
    #[cfg(test)]
    pub(crate) fn laid_afresh(&self) -> usize {
        self.laid.borrow().laid
    }

    /// Something changed that may change how lines are laid out: the next draw lays out
    /// afresh what it shows.
    pub fn changed(&mut self) {
        self.changes += 1;
    }

    /// Registration finished: ask for the channels, join them and fetch history.
    pub fn connected(&mut self, link: Link) -> Vec<Line> {
        self.changed();
        self.connection = Status::Connected {
            since: (self.chat.clock)(),
        };
        self.chat.connected(link)
    }

    /// Connecting again, after the connection was lost.
    pub fn connecting(&mut self) {
        self.chat.connecting();
    }

    /// The connection ended, and why.
    pub fn disconnected(&mut self, why: &str) {
        let now = (self.chat.clock)();
        let since = match &self.connection {
            Status::Offline { since, .. } => *since,
            _ => now,
        };
        self.connection = Status::Offline {
            since,
            why: why.to_owned(),
            retry_at: now,
            was_up: self.connection.was_up(),
        };
        self.chat.disconnected(why);
    }

    /// Takes in a line from the server, saying what to send back.
    pub fn handle(&mut self, line: Line) -> Vec<Line> {
        self.changed();
        let lines = self.chat.receive(line);
        // An open search finds what just came in.
        if let Some(searching) = &mut self.searching {
            searching.refresh(&self.chat, self.zone);
        }
        lines
    }

    /// Shows the conversation at `at`, marking it read. Returns what to send: where it's
    /// been read up to, for the person's other apps.
    #[must_use = "it says where the conversation's been read up to"]
    pub fn select(&mut self, at: usize) -> Vec<Line> {
        self.changed();
        // The server's, from the list, is the Server channel.
        if self
            .conversations()
            .get(at)
            .is_some_and(|c| c.kind == Kind::Server)
        {
            self.chat.show_server();
            return Vec::new();
        }
        // What's written stays with the conversation it was written in, and the one shown
        // next starts from its own (zirc-chat's drafts); a correction is let go.
        if self.editing.is_some() {
            self.stop_editing();
        } else {
            self.chat.set_draft(self.input.text());
        }
        let lines = self.chat.select(at);
        let draft = self.chat.draft().to_owned();
        let end = draft.len();
        self.input.replace(draft, end);
        lines
    }

    /// Whose profile is shown, if anyone's.
    #[must_use]
    pub fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    /// Shows `nick`'s profile, asking the server for what it knows of them. Returns what to
    /// send.
    fn show_profile(&mut self, nick: &str) -> Vec<Line> {
        self.profile = Some(nick.to_owned());
        self.chat.whois(nick)
    }

    /// The agent session whose card is shown, if any: its machine and its id.
    #[must_use]
    pub fn agent(&self) -> Option<(&str, &str)> {
        self.agent
            .as_ref()
            .map(|(nick, session)| (nick.as_str(), session.as_str()))
    }

    /// Shows the card of the agent session going by `handle`, or with that id, in a channel
    /// we're in; or says there's none. Sends nothing.
    fn show_agent(&mut self, handle: &str) -> Vec<Line> {
        match self.chat.session_named(handle) {
            Some(agent) => self.agent = Some(agent),
            None => self.note(format!("No agent session goes by {handle} here")),
        }
        Vec::new()
    }

    /// Says `text` in the conversation shown, as for an error.
    pub fn note(&mut self, text: String) {
        self.changed();
        self.chat.note(text);
    }

    /// What the last key sent wasn't, `why`. What was typed goes back in the input, and one
    /// line says so and what to do, as every app says a failed send.
    pub fn unsent(&mut self, why: &str) {
        match self.sent_typed.take() {
            None => self.note(format!("Not sent, {why}")),
            Some(typed) if self.input.text().is_empty() => {
                self.input.replace(typed.clone(), typed.len());
                self.note(format!(
                    "Not sent, {why}: it's back in the input, Enter sends it again"
                ));
            }
            // Something else typed since: the words not sent, said whole, to copy.
            Some(typed) => self.note(format!("Not sent, {why}: {typed}")),
        }
    }

    /// The scrollback of the conversation shown, as laid out at the size last drawn.
    #[must_use]
    pub fn rows(&self) -> Vec<Row> {
        self.rows_at(usize::from(self.view.0))
    }

    /// The scrollback of the conversation shown, laid out `width` columns wide.
    #[must_use]
    pub fn rows_at(&self, width: usize) -> Vec<Row> {
        self.laid(width).all()
    }

    /// The conversation shown, as zirc-chat's lines of it, to lay out `width` columns wide as
    /// it's looked at, each line's rows kept until it changes: an agent session's own as its
    /// activity everywhere ([`zirc_core::chat::Chat::agent_lines`]), as every app shows it.
    pub(crate) fn laid(&self, width: usize) -> Laid<'_, '_> {
        let today = self.zone.date((self.chat.clock)());
        let name = &self.current().name;
        let people = self.people_only();
        let look = rows::Look::new(
            &self.chat,
            &self.pressed,
            self.chat.divider(),
            width,
            self.links,
        );
        self.laid.borrow_mut().stamp((
            fold(name),
            width,
            look.text_width,
            self.links,
            people,
            today,
        ));
        let offset = self.zone.offset();
        let lines = if zirc_core::chat::agent(name).is_some() {
            self.chat.agent_lines(name, offset)
        } else {
            self.chat.lines(name, offset, people)
        };
        Laid::new(look, lines, &self.laid)
    }

    /// What the scrollback shows of `laid`, `height` rows high: from where it's read, or
    /// the newest rows.
    pub(crate) fn view_of(&self, laid: &Laid<'_, '_>, height: usize) -> View {
        let top = self
            .scrolled
            .get(&fold(&self.current().name))
            .and_then(|anchor| laid.place(anchor));
        laid.view(top, height)
    }

    /// The rows the scrollback, `width` by `height`, shows from the top, `None` for the
    /// blank ones above the oldest.
    #[must_use]
    pub fn shown_rows(&self, width: usize, height: usize) -> Vec<Option<Row>> {
        let laid = self.laid(width);
        self.view_of(&laid, height).rows
    }

    /// The picked message's details while they're shown (`i`), and how many of their lines
    /// are scrolled past.
    #[must_use]
    pub fn details(&self) -> Option<(&Details, u16)> {
        self.details
            .as_ref()
            .map(|(details, scrolled)| (details, *scrolled))
    }

    /// The id of the message picked to react to in the conversation shown, if one is.
    #[must_use]
    pub fn picked(&self) -> Option<&str> {
        self.picked
            .as_ref()
            .filter(|(name, _)| *name == fold(&self.current().name))
            .map(|(_, id)| id.as_str())
    }

    /// The reaction picker, when it's open.
    #[must_use]
    pub fn picker(&self) -> Option<&Picker> {
        self.picker.as_ref()
    }

    /// Jump to, when it's open.
    #[must_use]
    pub fn jumping(&self) -> Option<&Jumping> {
        self.jumping.as_ref()
    }

    /// Search, when it's open.
    #[must_use]
    pub fn searching(&self) -> Option<&Searching> {
        self.searching.as_ref()
    }

    /// The searches made lately, as the empty search box lists them.
    #[must_use]
    pub fn recent_searches(&self) -> Vec<zirc_core::chat::Recent> {
        self.recent_searches.listed()
    }

    /// The search screen open, as zirc-chat lays it out.
    #[must_use]
    pub fn search_screen(&self) -> Option<&SearchScreen> {
        self.searching.as_ref().map(Searching::screen)
    }

    /// Takes in a key while search is open. Returns what to send.
    fn searching_key(&mut self, key: KeyEvent) -> Vec<Line> {
        let recent = self.recent_searches();
        let Some(searching) = self.searching.as_mut() else {
            return Vec::new();
        };
        match searching.key(key, (&self.chat, self.zone), &recent) {
            Searched::Open => Vec::new(),
            Searched::Closed => {
                self.searching = None;
                Vec::new()
            }
            Searched::Hit(hit) => {
                // Opening what it found keeps the search among the recent ones.
                if let Some(searching) = &self.searching {
                    self.recent_searches.remember(&searching.finding.query());
                }
                self.prefs_changed = true;
                let mut lines = self.show_named(Some(hit.conversation.clone()));
                if let Some(id) = hit.id.filter(|_| hit.loaded) {
                    self.picked = Some((fold(&hit.conversation), id.clone()));
                    lines.extend(self.keep_in_view(&id));
                }
                lines
            }
        }
    }

    /// Takes in a key while Jump to is open. Returns what to send.
    fn jumping_key(&mut self, key: KeyEvent) -> Vec<Line> {
        let Some(jumping) = self.jumping.as_mut() else {
            return Vec::new();
        };
        match jumping.key(key, &self.chat) {
            Jumped::Open => Vec::new(),
            Jumped::Closed => {
                self.jumping = None;
                Vec::new()
            }
            Jumped::To(name) => {
                self.jumping = None;
                if self.chat.position(&name).is_some() {
                    self.show_named(Some(name))
                } else {
                    self.chat.open(&name)
                }
            }
        }
    }

    /// Picks the message `by` places on from the one picked, older when negative, and keeps
    /// it in view. From none, the first step up picks the newest; past the newest, none; and
    /// the oldest stays picked.
    fn pick(&mut self, by: isize) -> Vec<Line> {
        let ids: Vec<String> = {
            let laid = self.laid(usize::from(self.view.0));
            laid.lines()
                .iter()
                .filter_map(rows::line_id)
                .map(str::to_owned)
                .collect()
        };
        let here = self
            .picked()
            .and_then(|id| ids.iter().position(|i| i == id));
        let next = match here {
            Some(at) => Some(at.saturating_add_signed(by)).filter(|&at| at < ids.len()),
            None if by < 0 => ids.len().checked_sub(1),
            None => None,
        };
        let name = fold(&self.current().name);
        // What was revealed in the message picked before hides again.
        self.chat.hide_sealed();
        self.picked = next.map(|at| (name, ids[at].clone()));
        match self.picked().map(str::to_owned) {
            Some(id) => self.keep_in_view(&id),
            None => Vec::new(),
        }
    }

    /// Scrolls so the rows of the message `id` are all in view. Returns what to send: the
    /// page of history before, when that's near.
    fn keep_in_view(&mut self, id: &str) -> Vec<Line> {
        let height = usize::from(self.view.1).max(1);
        let (anchor, older) = {
            let laid = self.laid(usize::from(self.view.0));
            let Some(at) = laid
                .lines()
                .iter()
                .position(|line| rows::line_id(line) == Some(id))
            else {
                return Vec::new();
            };
            let len = laid.rows(at).len();
            if len == 0 {
                return Vec::new();
            }
            let view = self.view_of(&laid, height);
            let Some(top) = view.top else {
                return Vec::new();
            };
            let bottom = laid.step(top, isize::try_from(height).unwrap_or(isize::MAX) - 1);
            let top = if (at, 0) < top {
                (at, 0)
            } else if (at, len - 1) > bottom {
                laid.step(
                    (at, len - 1),
                    1 - isize::try_from(height).unwrap_or(isize::MAX),
                )
            } else {
                return Vec::new();
            };
            let at_end = laid.view(Some(top), height).at_end;
            (
                (!at_end).then(|| laid.anchor(top)),
                laid.wants_older(top, height),
            )
        };
        self.anchor(anchor, older)
    }

    /// Holds the conversation shown at `anchor`, or at the end, and asks for the page before
    /// when it's `older` and there is one. Returns what to send.
    fn anchor(&mut self, anchor: Option<Anchor>, older: bool) -> Vec<Line> {
        let name = fold(&self.current().name);
        match anchor {
            Some(anchor) => {
                self.scrolled.insert(name, anchor);
            }
            None => {
                self.scrolled.remove(&name);
            }
        }
        if !older || !self.scrolled_back() {
            return Vec::new();
        }
        let name = self.current().name.clone();
        self.chat.older(&name).into_iter().collect()
    }

    /// While the conversation shown is scrolled back near the top of what it holds, asks
    /// for the page of history before, as when it's scrolled there: after older history
    /// came in short of filling what's above. Returns what to send.
    pub fn older_if_near_top(&mut self) -> Vec<Line> {
        if !self.scrolled_back() {
            return Vec::new();
        }
        let height = usize::from(self.view.1).max(1);
        let near = {
            let laid = self.laid(usize::from(self.view.0));
            let view = self.view_of(&laid, height);
            view.top
                .filter(|_| !view.at_end)
                .is_some_and(|top| laid.wants_older(top, height))
        };
        let name = self.current().name.clone();
        if near {
            self.chat.older(&name).into_iter().collect()
        } else {
            Vec::new()
        }
    }

    /// What the next line sent answers, in the conversation shown, if it's a reply.
    #[must_use]
    pub fn replying(&self) -> Option<Quote> {
        let (name, id) = self.replying.as_ref()?;
        let conversation = self.current();
        if *name != fold(&conversation.name) {
            return None;
        }
        let reply = Entry {
            reply_to: Some(id.clone()),
            ..Entry::info((self.chat.clock)(), String::new())
        };
        self.chat.quote(conversation, &reply)
    }

    /// Starts a reply to the message `id` in the conversation shown, which the next line
    /// sent answers.
    fn reply_to(&mut self, id: String) {
        self.stop_editing();
        self.replying = Some((fold(&self.current().name), id));
        self.let_go();
    }

    /// Whether the next line sent corrects a message of ours in the conversation shown.
    #[must_use]
    pub fn editing(&self) -> bool {
        self.editing_id().is_some()
    }

    /// The message of ours the next line sent corrects, in the conversation shown.
    fn editing_id(&self) -> Option<&str> {
        let (name, id, _) = self.editing.as_ref()?;
        (*name == fold(&self.current().name)).then_some(id.as_str())
    }

    /// What follows Editing while a message of ours in the conversation shown is being
    /// corrected: `: ` and its first line, as every app says it ([`zirc_core::chat::editing_after`]).
    #[must_use]
    pub fn editing_after(&self) -> Option<String> {
        let (name, _, said) = self.editing.as_ref()?;
        (*name == fold(&self.current().name)).then(|| zirc_core::chat::editing_after(said))
    }

    /// Up, with `held`: in an empty input, corrects our last message here, as in every app.
    fn up(&mut self, held: KeyModifiers) {
        let composer = keys::Composing {
            empty: self.input.text().is_empty(),
            editing: self.editing(),
        };
        if keys::up_edits_last(composer, pressed(held)) {
            self.edit_last();
        }
    }

    /// Corrects our last message in the conversation shown, the core's pick, as Up in an empty
    /// input does in every app; nothing when there's none.
    fn edit_last(&mut self) {
        let name = self.current().name.clone();
        if let Some(id) = self.chat.last_editable(&name) {
            self.edit(id);
        }
    }

    /// Puts our message `id` in the conversation shown in the input to correct it, while
    /// it's ours to correct: the next line sent takes its place.
    fn edit(&mut self, id: String) {
        let name = self.current().name.clone();
        let Some(text) = self.chat.edit_text(&name, &id) else {
            self.note(zirc_core::chat::TOO_LATE_TO_EDIT.to_owned());
            return;
        };
        self.replying = None;
        self.let_go();
        let end = text.len();
        self.editing = Some((fold(&name), id, text.clone()));
        self.input.replace(text, end);
    }

    /// Stops correcting a message or writing a reply, which Esc lets go of first.
    fn stop_composing(&mut self) {
        self.stop_editing();
        self.replying = None;
    }

    /// Stops correcting a message, emptying the input of its words.
    fn stop_editing(&mut self) {
        if self.editing.take().is_some() {
            self.input.replace(String::new(), 0);
        }
    }

    /// `/reply` alone: starts a reply to the message picked, or the newest, that the next
    /// line sends.
    fn reply(&mut self) -> Vec<Line> {
        match self.reacting_to() {
            Some(id) => {
                self.reply_to(id);
                Vec::new()
            }
            None => self.refuse(said::NOTHING_TO_REPLY),
        }
    }

    /// What to put on the clipboard, if a key copied something since it was last taken.
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.clipboard.take()
    }

    /// What the last key did, if it's worth saying, as the status line says it: what it
    /// copied, or pinned.
    #[must_use]
    pub fn flash(&self) -> Option<&str> {
        self.flash.as_deref()
    }

    /// What the client keeps for the person.
    #[must_use]
    pub fn prefs(&self) -> Prefs {
        Prefs {
            pins: self.chat.stars().to_vec(),
            send_typing: self.send_typing,
            editor: self.editor.clone(),
            people_only: self.people_only.iter().cloned().collect(),
            thresholds: self
                .chat
                .thresholds()
                .into_iter()
                .map(|(name, level)| (name, level.to_string()))
                .collect(),
            closed: self
                .chat
                .closed()
                .into_iter()
                .map(|closed| (closed.name, closed.at_ms))
                .collect(),
            recent_searches: [(self.server().to_owned(), self.recent_searches.0.clone())]
                .into_iter()
                .filter(|(_, recent)| !recent.is_empty())
                .collect(),
        }
    }

    /// Takes up what was kept for the person.
    pub fn use_prefs(&mut self, prefs: Prefs) {
        self.changed();
        self.chat.set_stars(prefs.pins);
        self.send_typing = prefs.send_typing;
        self.chat.set_send_typing(prefs.send_typing);
        self.chat
            .set_editor(zirc_core::editor::editor(prefs.editor.as_deref()));
        self.editor = prefs.editor;
        self.people_only = prefs.people_only.iter().map(|name| fold(name)).collect();
        self.chat.set_thresholds(
            prefs
                .thresholds
                .into_iter()
                .filter_map(|(name, level)| Some((name, level.parse().ok()?))),
        );
        self.chat.set_closed(
            prefs
                .closed
                .into_iter()
                .map(|(name, at_ms)| zirc_core::chat::Closed { name, at_ms }),
        );
        self.recent_searches = RecentSearches(
            prefs
                .recent_searches
                .get(self.server())
                .cloned()
                .unwrap_or_default(),
        );
    }

    /// What to keep for the person, if it changed since it was last taken.
    pub fn take_prefs(&mut self) -> Option<Prefs> {
        std::mem::take(&mut self.prefs_changed).then(|| self.prefs())
    }

    /// Pins the conversation shown to the top of its section, or unpins it.
    fn toggle_pin(&mut self) {
        let conversation = self.current();
        if conversation.kind == Kind::Server {
            return;
        }
        let name = conversation.name.clone();
        let title = self.chat.title(&name);
        let pinned = !self.chat.is_starred(&name);
        self.chat.star(&name, pinned);
        self.prefs_changed = true;
        self.flash = Some(if pinned {
            format!("pinned {title}, Alt-P unpins")
        } else {
            format!("unpinned {title}")
        });
    }

    /// Copies the message picked, or with `link` the first link in it.
    fn copy(&mut self, link: bool) {
        let Some(id) = self.picked() else {
            return;
        };
        let Some(entry) = self
            .current()
            .entries
            .iter()
            .find(|entry| entry.id.as_deref() == Some(id))
        else {
            return;
        };
        // Sealed parts as shown: their values only once revealed.
        let text = self.chat.sealed_text(entry, &copy_text(entry));
        let (copied, said) = if link {
            match first_link(&text) {
                Some(link) => (Some(link.to_owned()), "copied its link"),
                None => (None, "no link in it to copy"),
            }
        } else {
            (Some(text), "copied the message")
        };
        if copied.is_some() {
            self.clipboard = copied;
        }
        self.flash = Some(said.to_owned());
    }

    /// Opens the picked message when it's long and cut, or cuts it again, as its control's
    /// press does in every app ([`zirc_core::chat::Chat::toggle_more`]). Whether it's long.
    fn toggle_more(&mut self) -> bool {
        let Some(id) = self.picked().map(str::to_owned) else {
            return false;
        };
        let more = {
            let laid = self.laid(usize::from(self.view.0));
            laid.lines()
                .iter()
                .find(|line| line.id.as_deref() == Some(id.as_str()))
                .and_then(|line| Some((line.key.clone(), line.more.as_ref()?.open)))
        };
        let Some((key, open)) = more else {
            return false;
        };
        self.changed();
        self.chat.toggle_more(&key, open);
        true
    }

    /// Reveals the picked message's sealed parts, or hides them again.
    fn reveal(&mut self) {
        let Some(id) = self.picked() else {
            return;
        };
        let Some(entry) = self
            .current()
            .entries
            .iter()
            .find(|entry| entry.id.as_deref() == Some(id))
            .cloned()
        else {
            return;
        };
        if self.chat.sealed_parts(&entry).is_empty() {
            self.flash = Some("no sealed parts in it".to_owned());
        } else {
            self.chat.toggle_sealed(&entry);
        }
    }

    /// Takes in a key while a profile is shown: Enter talks to them, Esc closes it, and
    /// anything else closes it and goes on as usual (`None`).
    fn profile_key(&mut self, key: KeyEvent) -> Option<Action> {
        let nick = self.profile.take()?;
        match key.code {
            KeyCode::Esc => Some(Action::Send(Vec::new())),
            KeyCode::Enter if !nick.eq_ignore_ascii_case(self.nick()) => {
                Some(Action::Send(self.chat.open(&nick)))
            }
            _ => None,
        }
    }

    /// Takes in a key while a message's details are shown: ↑/↓ and PgUp/PgDn scroll them,
    /// anything else closes them. `None` while none are shown.
    fn details_key(&mut self, key: KeyEvent, ctrl: bool) -> Option<Action> {
        let (_, scrolled) = self.details.as_mut()?;
        match key.code {
            KeyCode::Up => *scrolled = scrolled.saturating_sub(1),
            KeyCode::Down => *scrolled = scrolled.saturating_add(1),
            KeyCode::PageUp => *scrolled = scrolled.saturating_sub(10),
            KeyCode::PageDown => *scrolled = scrolled.saturating_add(10),
            KeyCode::Char('c') if ctrl => return Some(Action::Quit(vec![quit(None)])),
            _ => self.details = None,
        }
        Some(Action::Send(Vec::new()))
    }

    /// Lets go of the picked message, hiding any sealed part revealed in it.
    fn let_go(&mut self) {
        self.picked.take();
        self.chat.hide_sealed();
    }

    /// Takes in a key while a message is picked and nothing's typed: ↑ and ↓ pick another,
    /// `r` starts a reply to it, `e` corrects it when it's ours, `y` copies it and `l` its
    /// link, `s` shows or hides its sealed parts, `i` shows its details. Returns what to send,
    /// if it was one of those.
    fn picked_key(&mut self, key: KeyEvent) -> Option<Vec<Line>> {
        match key.code {
            KeyCode::Up => return Some(self.pick(-1)),
            KeyCode::Down => return Some(self.pick(1)),
            KeyCode::Char('r') => {
                if let Some(id) = self.picked().map(str::to_owned) {
                    self.reply_to(id);
                }
            }
            KeyCode::Char('e') => {
                if let Some(id) = self.picked().map(str::to_owned) {
                    self.edit(id);
                }
            }
            KeyCode::Char('y') => self.copy(false),
            KeyCode::Char('l') => self.copy(true),
            KeyCode::Char('s') => self.reveal(),
            // Enter on a long message opens it, or cuts it again, as its row says.
            KeyCode::Enter if self.toggle_more() => {}
            KeyCode::Char('i') => {
                let name = self.current().name.clone();
                self.details = self
                    .picked()
                    .and_then(|id| self.chat.details(&name, id))
                    .map(|details| (details, 0));
            }
            _ => return None,
        }
        Some(Vec::new())
    }

    /// The message about to be trimmed, waiting for the person to confirm it.
    #[must_use]
    pub fn trimming(&self) -> Option<&Trimming> {
        self.trimming.as_ref()
    }

    /// Asks the person to confirm trimming the message a reaction would go to.
    fn ask_to_trim(&mut self, reason: Option<String>) {
        if !self.chat.link().is_some_and(|link| link.trim) {
            let server = self.server().to_owned();
            self.note(format!(
                "{server} doesn't let messages be trimmed from its history"
            ));
            return;
        }
        let conversation = self.current();
        let entry = self.reacting_to().and_then(|id| {
            conversation
                .entries
                .iter()
                .find(|entry| entry.id.as_deref() == Some(id.as_str()))
        });
        match entry {
            Some(entry) => {
                self.trimming = Some(Trimming {
                    target: conversation.name.clone(),
                    id: entry.id.clone().unwrap_or_default(),
                    text: entry_text(entry),
                    reason,
                });
            }
            None => self.note("nothing here to trim".into()),
        }
    }

    /// Takes in a key while a trim waits to be confirmed: `y` trims, anything else keeps
    /// the message. Returns what to send.
    fn trimming_key(&mut self, key: KeyEvent) -> Vec<Line> {
        let Some(trimming) = self.trimming.take() else {
            return Vec::new();
        };
        if !matches!(key.code, KeyCode::Char('y' | 'Y')) {
            return Vec::new();
        }
        self.let_go();
        self.chat
            .trim(&trimming.target, &trimming.id, trimming.reason.as_deref())
    }

    /// The message a reaction or a trim goes to: the one picked, or else the newest.
    fn reacting_to(&self) -> Option<String> {
        self.picked().map(str::to_owned).or_else(|| {
            let laid = self.laid(usize::from(self.view.0));
            laid.lines()
                .iter()
                .rev()
                .find_map(rows::line_id)
                .map(str::to_owned)
        })
    }

    /// Gives `emoji` to the message `id` in the conversation shown, or takes it back if we
    /// gave it already. Returns what to send.
    fn react(&mut self, id: &str, emoji: &str) -> Vec<Line> {
        self.recent.used(emoji);
        self.chat.react(&self.current().name, id, emoji)
    }

    /// Opens the picker on the message a reaction would go to.
    fn open_picker(&mut self) {
        match self.reacting_to() {
            Some(id) => self.picker = Some(Picker::new(&self.current().name, &id)),
            None => self.note("nothing here to react to yet".into()),
        }
    }

    /// Takes in a key while the picker's open. Returns what to send.
    fn picker_key(&mut self, key: KeyEvent) -> Vec<Line> {
        let Some(picker) = self.picker.as_mut() else {
            return Vec::new();
        };
        match picker.key(key, &self.recent) {
            Picked::Open => Vec::new(),
            Picked::Closed => {
                self.picker = None;
                Vec::new()
            }
            Picked::Emoji(emoji) => {
                let picker = self.picker.take().expect("open");
                self.recent.used(emoji);
                self.chat.react(&picker.target, &picker.id, emoji)
            }
        }
    }

    /// Enter with `held`, as zirc-chat says for every app ([`keys::enter`]): it sends; with
    /// Shift, Alt, Ctrl or Super, a new line.
    fn enter(&mut self, held: KeyModifiers) -> Action {
        match keys::enter(pressed(held)) {
            keys::EnterDoes::Send => self.submit(),
            keys::EnterDoes::NewLine | keys::EnterDoes::Composing => {
                self.input.insert('\n');
                Action::Send(Vec::new())
            }
        }
    }

    /// Takes in a key, saying what to send.
    pub fn key(&mut self, key: KeyEvent) -> Action {
        if !lays_out_the_same(key) {
            self.changed();
        }
        self.take_key(key)
    }

    /// Takes in a key while a card that takes every key is open, the reaction picker, Jump
    /// to or the trim card: Ctrl-C still quits.
    fn card_key(&mut self, key: KeyEvent, ctrl: bool) -> Option<Action> {
        if self.picker.is_none()
            && self.jumping.is_none()
            && self.searching.is_none()
            && self.trimming.is_none()
        {
            return None;
        }
        if ctrl && key.code == KeyCode::Char('c') {
            return Some(Action::Quit(vec![quit(None)]));
        }
        Some(Action::Send(if self.picker.is_some() {
            self.picker_key(key)
        } else if self.jumping.is_some() {
            self.jumping_key(key)
        } else if self.searching.is_some() {
            self.searching_key(key)
        } else {
            self.trimming_key(key)
        }))
    }

    fn take_key(&mut self, key: KeyEvent) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::Send(Vec::new());
        }
        self.flash = None;
        self.sent_typed = None;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        if let Some(action) = self.card_key(key, ctrl) {
            return action;
        }
        if let Some(action) = self
            .details_key(key, ctrl)
            .or_else(|| self.leaving_key(key))
            .or_else(|| self.pressing_key(key))
            .or_else(|| self.profile_key(key))
        {
            return action;
        }
        // The sessions panel: Esc closes it, and anything else closes it and goes on. Browse
        // channels stays while a /join is typed, and Esc closes it.
        let sessions = self.sessions.take().is_some() | self.agent.take().is_some();
        if key.code == KeyCode::Esc && (sessions || self.browsing.take().is_some()) {
            return Action::Send(Vec::new());
        }
        if self.picked().is_some()
            && self.input.text().is_empty()
            && !ctrl
            && !alt
            && let Some(lines) = self.picked_key(key)
        {
            return Action::Send(lines);
        }
        let before = self.input.text().to_owned();
        match key.code {
            KeyCode::Char('c') if ctrl => return Action::Quit(vec![quit(None)]),
            KeyCode::Char('d') if ctrl && self.input.text().is_empty() => {
                return Action::Quit(vec![quit(None)]);
            }
            // What Enter does is zirc-chat's, as in every app (keys::enter): it sends; with
            // Shift, Alt, Ctrl or Super, a new line. Alt+Enter is the one a terminal that can't
            // tell Shift+Enter from Enter still sends us.
            KeyCode::Enter => return self.enter(key.modifiers),
            KeyCode::Char('n') if ctrl => return Action::Send(self.cycle(1)),
            KeyCode::Char('p') if ctrl => return Action::Send(self.cycle(-1)),
            KeyCode::Down if alt || ctrl => return Action::Send(self.cycle(1)),
            KeyCode::Up if alt || ctrl => return Action::Send(self.cycle(-1)),
            KeyCode::Up if shift => return Action::Send(self.pick(-1)),
            KeyCode::Down if shift => return Action::Send(self.pick(1)),
            KeyCode::Char('r' | 'R') if alt => self.open_picker(),
            KeyCode::Char('k' | 'K') if alt => self.jumping = Some(Jumping::default()),
            KeyCode::Char('f' | 'F') if alt => {
                let shown = self.current().name.clone();
                self.searching = Some(Searching::new(&self.chat, Some(shown), self.zone));
            }
            KeyCode::Char('p' | 'P') if alt => self.toggle_pin(),
            KeyCode::Char('o' | 'O') if alt && self.orgs.len() > 1 => return Action::Org(None),
            KeyCode::Char('s' | 'S') if alt => return self.open_sessions(),
            // Shift-Esc marks everything read. Esc lets a reply or a correction go first,
            // then the message picked, and else marks the conversation shown read.
            KeyCode::Esc if shift => return Action::Send(self.chat.mark_all_read()),
            KeyCode::Esc if self.editing() || self.replying().is_some() => self.stop_composing(),
            KeyCode::Esc if self.picked().is_some() => self.let_go(),
            KeyCode::Esc => {
                let name = self.current().name.clone();
                return Action::Send(self.chat.mark_read(&name));
            }
            KeyCode::Char('a') if alt => {
                return Action::Send(self.next_unread());
            }
            KeyCode::Char(c @ '1'..='9') if alt => {
                return Action::Send(self.show_listed(c as usize - '1' as usize));
            }
            KeyCode::Char('0') if alt => return Action::Send(self.show_listed(9)),
            KeyCode::Char('a') if ctrl => self.input.home(),
            KeyCode::Char('e') if ctrl => self.input.end(),
            KeyCode::Char('u') if ctrl => self.input.kill_to_start(),
            KeyCode::Char('k') if ctrl => self.input.kill_to_end(),
            KeyCode::Char('w') if ctrl => self.input.kill_word(),
            KeyCode::Char(c) if !ctrl && !alt => self.input.insert(c),
            KeyCode::Backspace if alt => self.input.kill_word(),
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            KeyCode::Up => self.up(key.modifiers),
            KeyCode::Tab => self.complete(),
            KeyCode::PageUp => return Action::Send(self.scroll_up()),
            KeyCode::PageDown => return Action::Send(self.scroll_down()),
            _ => {}
        }
        if !self.edited(&before) {
            return Action::Send(Vec::new());
        }
        Action::Send(self.typed())
    }

    /// Whether a key changed the input from `before`. A `:shortcode:` it closed becomes its
    /// emoji first, as zirc-core writes it.
    fn edited(&mut self, before: &str) -> bool {
        if let Some((text, at)) = emoji::closed_shortcode(
            before,
            self.input.text(),
            self.input.cursor_byte(),
            picker::NEWEST,
        ) {
            self.input.replace(text, at);
        }
        self.input.text() != before
    }

    /// What the input holding something else says to the conversation shown: that we're
    /// typing, or with it cleared, that we stopped. Not for a command, nor where there's
    /// no one to tell.
    fn typed(&mut self) -> Vec<Line> {
        let text = self.input.text();
        let command = text.starts_with('/') && !text.starts_with("//");
        let conversation = self.current();
        if command || conversation.kind == Kind::Server {
            return Vec::new();
        }
        let (name, empty) = (conversation.name.clone(), text.is_empty());
        self.chat.typed(&name, empty)
    }

    /// Who's typing in the conversation shown, as the line above the input says it.
    #[must_use]
    pub fn typing(&self) -> Option<String> {
        self.chat.typing_text(&self.current().name)
    }

    /// `/typing [on|off]`: says, or sets, whether others are told we're typing. Returns
    /// what to send: nothing.
    fn set_send_typing(&mut self, on: Option<bool>) -> Vec<Line> {
        if let Some(on) = on {
            self.send_typing = on;
            self.chat.set_send_typing(on);
            self.prefs_changed = true;
        }
        self.note(if self.send_typing {
            "others see when you're typing: /typing off stops it".into()
        } else {
            "others don't see when you're typing: /typing on lets them".into()
        });
        Vec::new()
    }

    /// Takes in a click or a turn of the wheel at `target`, saying what to send. A click on
    /// a conversation shows it, and on someone in the member list opens a private one with
    /// them; the wheel scrolls the scrollback.
    pub fn pointer(&mut self, kind: MouseEventKind, target: Option<Target>) -> Action {
        self.sent_typed = None;
        if !matches!(
            kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown | MouseEventKind::Moved
        ) {
            self.changed();
        }
        let lines = match (kind, target) {
            (MouseEventKind::Down(MouseButton::Left), Some(Target::Conversation(at))) => {
                self.select(at)
            }
            (
                MouseEventKind::Down(MouseButton::Left),
                Some(Target::Agent(name) | Target::Open(name)),
            ) => self.chat.open(&name),
            (MouseEventKind::Down(MouseButton::Left), Some(Target::GuestRooms(channel))) => {
                let folded = self
                    .chat
                    .channel_groups()
                    .into_iter()
                    .find(|group| group.name.eq_ignore_ascii_case(&channel))
                    .is_some_and(|group| group.guests.folded);
                self.chat.set_guest_rooms_open(&channel, folded);
                Vec::new()
            }
            (MouseEventKind::Down(MouseButton::Left), Some(Target::Member(nick))) => {
                self.show_profile(&nick)
            }
            (MouseEventKind::Down(MouseButton::Left), Some(Target::Tab(tab))) => {
                self.chat.set_tab(tab);
                Vec::new()
            }
            (MouseEventKind::Down(MouseButton::Left), Some(Target::Shelf(part))) => {
                let folded = self.chat.collapsed().contains(&part);
                self.chat.set_collapsed(part, !folded);
                Vec::new()
            }
            (MouseEventKind::Down(MouseButton::Left), Some(Target::Message(id))) => {
                self.picked = Some((fold(&self.current().name), id));
                Vec::new()
            }
            (MouseEventKind::Down(MouseButton::Left), Some(Target::Reaction { id, emoji })) => {
                self.react(&id, &emoji)
            }
            (MouseEventKind::ScrollUp, _) => self.scroll_by(WHEEL_ROWS.cast_signed()),
            (MouseEventKind::ScrollDown, _) => self.scroll_by(-WHEEL_ROWS.cast_signed()),
            _ => Vec::new(),
        };
        Action::Send(lines)
    }

    /// `/decide`: our decision about `about`, in our words, `quote`, sent from our own
    /// account to `#decisions`, joined if need be, and to `here` when it's another channel.
    /// Returns what to send.
    fn decide(&mut self, about: String, quote: String, here: &str) -> Vec<Line> {
        let me = self.nick().to_owned();
        let decision = zirc_proto::decision::Decision {
            by: me.clone(),
            quote,
            about,
            session: None,
            link: None,
            for_: Vec::new(),
            replaces: None,
        };
        let board = zirc_core::decisions::CHANNEL;
        let mut lines = Vec::new();
        if !self.chat.conversation(board).is_some_and(|c| c.joined) {
            lines.push(Line::new("JOIN", [board]));
        }
        lines.extend(self.chat.post_event(board, &me, decision.event()));
        if is_channel(here) && !here.eq_ignore_ascii_case(board) {
            lines.extend(self.chat.post_event(here, &me, decision.event()));
        }
        self.note(format!("decided: {}", decision.text()));
        lines
    }

    /// `/agents`: the agent sessions on our machines, each a name to `/query`.
    /// What to ask before leaving a channel, zirc-chat's words, while `/part` waits on y.
    #[must_use]
    pub fn leaving(&self) -> Option<String> {
        self.leaving
            .as_ref()
            .map(|(channel, _)| self.chat.leave_prompt(channel))
    }

    /// `/browse`: opens Browse channels over the scrollback, the channels we aren't in as
    /// zirc-chat's `Chat::browse` lists them, until Esc or the next line sent.
    fn browse(&mut self) -> Vec<Line> {
        self.browsing = Some(BrowsePanel);
        Vec::new()
    }

    /// Whether Browse channels is open.
    #[must_use]
    pub fn browsing(&self) -> bool {
        self.browsing.is_some()
    }

    fn list_agents(&mut self) -> Vec<Line> {
        let agents = self.chat.agents();
        self.note(if agents.is_empty() {
            "no agents are working on your machines".to_owned()
        } else {
            format!("your agents, each to /query: {}", agents.join(", "))
        });
        Vec::new()
    }

    /// `/queries`: what zircd is to be asked about who may query this machine's agents. Letting
    /// anyone takes `yes` after the warning, which only says why first.
    fn queries(&mut self, words: Option<&str>) -> Action {
        let set = match words
            .map(str::split_whitespace)
            .map(Iterator::collect::<Vec<_>>)
        {
            None => None,
            Some(words) => match words.as_slice() {
                ["anyone"] => {
                    self.note(format!(
                        "{}\nTo let anyone all the same: /queries anyone yes",
                        agents::ANYONE_WARNING
                    ));
                    return Action::Send(Vec::new());
                }
                ["anyone", "yes"] => Some(Queries::Anyone),
                [word] if Queries::from_word(word).is_some() => Queries::from_word(word),
                _ => {
                    self.note("/queries takes off, mine or anyone".into());
                    return Action::Send(Vec::new());
                }
            },
        };
        Action::Zircd(Request::Queries { set })
    }

    /// What this machine's zircd answered, or why it couldn't, said where the person is
    /// looking.
    pub fn zircd_answered(&mut self, answer: Result<Response, String>) {
        self.changed();
        let said = match answer {
            Ok(Response::Repos { repos }) => {
                self.chat
                    .set_checkouts(zirc_core::editor::checkouts(&repos));
                return;
            }
            Ok(Response::Queries {
                queries,
                colleagues_kept_out,
            }) => agents::said(queries, colleagues_kept_out),
            Ok(Response::Error { message }) | Err(message) => format!("zircd: {message}"),
            Ok(other) => format!("zircd answered {other:?}"),
        };
        for line in said.lines() {
            self.note(line.to_owned());
        }
    }

    /// Opens the sessions panel, asking zircd what to show in it.
    fn open_sessions(&mut self) -> Action {
        self.sessions = Some(SessionsPanel {
            listed: None,
            at: 0,
        });
        Action::Sessions
    }

    /// The sessions panel, while it's open.
    #[must_use]
    pub fn sessions(&self) -> Option<&SessionsPanel> {
        self.sessions.as_ref()
    }

    /// What zircd said every session is doing, at `at`, or why it couldn't say: shown in
    /// the sessions panel, if it's still open.
    pub fn sessions_answered(
        &mut self,
        listed: Result<Vec<zirc_core::sessions::Session>, String>,
        at: u64,
    ) {
        if let Some(panel) = &mut self.sessions {
            panel.listed = Some(listed);
            panel.at = at;
        }
    }

    /// Shows the conversation `nth` in the list, if there is one. Returns what to send.
    fn show_listed(&mut self, nth: usize) -> Vec<Line> {
        // Alt-1 is the first conversation listed: the Server channel never is.
        let at = self.listed().nth(nth).map(|(at, _)| at);
        at.map(|at| self.select(at)).unwrap_or_default()
    }

    /// Next conversation (`by` 1) or Previous (`by` -1), down or up both halves of the
    /// list, round the ends, as zirc-chat steps for every app. Returns what to send.
    fn cycle(&mut self, by: isize) -> Vec<Line> {
        let next = self.chat.next_conversation(&ListOptions::default(), by);
        self.show_named(next)
    }

    /// Next unread: what mentions us first, round the ends, as zirc-chat says for every
    /// app. Returns what to send.
    fn next_unread(&mut self) -> Vec<Line> {
        let next = self.chat.next_unread(1);
        self.show_named(next)
    }

    /// Shows the conversation called `name`, if there's one. Returns what to send.
    fn show_named(&mut self, name: Option<String>) -> Vec<Line> {
        let at = name.and_then(|name| {
            self.conversations()
                .iter()
                .position(|conversation| conversation.name.eq_ignore_ascii_case(&name))
        });
        at.map(|at| self.select(at)).unwrap_or_default()
    }

    fn submit(&mut self) -> Action {
        let typed = self.input.take();
        if !zirc_core::compose::sendable(&typed) {
            return Action::Send(Vec::new());
        }
        self.browsing = None;
        self.back_to_end();
        let command = match command::parse(&typed) {
            Ok(command) => command,
            Err(message) => {
                // What was typed stays, to put right.
                self.input.replace(typed.clone(), typed.len());
                self.note(message.to_string());
                return Action::Send(Vec::new());
            }
        };
        self.sent_typed = Some(typed.clone());
        if let Some(feature) = command.feature() {
            zirc_core::telemetry::record(&Event::FeatureUsed(FeatureUsed { feature }));
        }
        self.run(command)
    }

    /// Carries out a command typed in the conversation shown: zirc-chat's, as every app
    /// carries it out ([`Chat::run`]), then this client's own.
    fn run(&mut self, command: Command) -> Action {
        let here = self.current().name.clone();
        if let Some(action) = self.guest_admin(&command, &here) {
            return action;
        }
        let picked = self.reacting_to();
        let replying = self.replying().map(|quote| quote.id);
        let editing = self.editing_id().map(str::to_owned);
        let ran = self.chat.run(
            &Here {
                name: &here,
                picked: picked.as_deref(),
                replying: replying.as_deref(),
                editing: editing.as_deref(),
                newest: picker::NEWEST,
            },
            command,
        );
        let lines = match ran {
            Ran::Send(lines) => lines,
            Ran::Edited(lines) => {
                self.editing = None;
                lines
            }
            Ran::Replied(lines) => {
                self.replying = None;
                self.let_go();
                lines
            }
            Ran::Reacted { emoji, lines } => {
                self.recent.used(emoji);
                lines
            }
            Ran::Card(nick) => self.show_profile(&nick),
            // Signed in with zirc.dev, a nick is the account's handle there, changed on its
            // account page alone, which opens as /profile does (20 Nov 2026), in every app.
            Ran::Nick(_) if self.site.is_some() => return self.profile_page(),
            Ran::Nick(nick) => vec![Line::new("NICK", [nick])],
            // A direct message stays closed when history brings it back, until something new
            // is said in it.
            Ran::CloseDirect(name) => {
                self.chat.close_direct(&name);
                self.prefs_changed = true;
                self.chat.close(self.selected())
            }
            // Asks before leaving, as every app asks.
            Ran::Leave { channel, reason } => {
                self.leaving = Some((channel, reason));
                Vec::new()
            }
            Ran::Note(words) => self.refuse(&words),
            Ran::App(command) => return self.run_here(command, &here),
        };
        Action::Send(lines)
    }

    /// Carries out a command zirc-chat leaves to the app, typed in `here`: this client's own.
    fn run_here(&mut self, command: Command, here: &str) -> Action {
        let lines = match command {
            Command::Queries(words) => return self.queries(words.as_deref()),
            Command::Decide { about, quote } => self.decide(about, quote, here),
            Command::Agents => self.list_agents(),
            Command::Browse => self.browse(),
            // A guest's, shown or named, is asked of zirc.dev first ([`Self::guest_admin`]).
            Command::Ban | Command::Unban(_) => self.refuse(said::FOR_GUESTS),
            Command::Who(handle) => self.show_agent(&handle),
            Command::React(None) => {
                self.open_picker();
                Vec::new()
            }
            Command::Reply(None) => self.reply(),
            Command::Trim(reason) => {
                self.ask_to_trim(reason);
                Vec::new()
            }
            Command::Quote(raw) => match raw.parse::<Line>() {
                Ok(line) => vec![line],
                Err(err) => {
                    self.note(err.to_string());
                    Vec::new()
                }
            },
            Command::Typing(on) => self.set_send_typing(on),
            Command::People => self.toggle_people(),
            Command::Show(choice) => self.show(choice.as_deref()),
            Command::Editor(choice) => self.choose_editor(choice.as_deref()),
            Command::Press(name) => return self.press(&name),
            Command::Sessions => return self.open_sessions(),
            Command::Help => self.help(),
            Command::Profile => return self.profile_page(),
            Command::Org(name) => return self.org(name),
            Command::Quit(reason) => return Action::Quit(vec![quit(reason.as_deref())]),
            other => self.refuse(&said::refused(&other)),
        };
        Action::Send(lines)
    }

    /// Whether another organisation is shown instead of this one: while it is, this one shows
    /// the server's conversation, which has nothing to mark read, so what's said elsewhere
    /// counts as unread; once it's shown again, it shows what it showed. Lines to send.
    pub fn hide(&mut self, hidden: bool) -> Vec<Line> {
        self.changed();
        let server = self
            .conversations()
            .iter()
            .position(|conversation| conversation.kind == Kind::Server);
        if hidden {
            let current = self.current();
            if current.kind == Kind::Server {
                return Vec::new();
            }
            let name = current.name.clone();
            // Kept from when it was first hidden, as joins can show another meanwhile.
            self.hidden.get_or_insert(name);
            // Shown as nothing, not as the Server channel.
            return server.map(|at| self.chat.select(at)).unwrap_or_default();
        }
        let Some(name) = self.hidden.take() else {
            return Vec::new();
        };
        match self.conversations().iter().position(|it| it.is(&name)) {
            Some(at) => self.select(at),
            None => Vec::new(),
        }
    }

    /// Says what the commands and keys are, with the organisations' on a client signed in to
    /// several.
    fn help(&mut self) -> Vec<Line> {
        let orgs = if self.orgs.len() > 1 {
            command::ORG_HELP
        } else {
            &[]
        };
        for line in command::HELP.iter().chain(orgs) {
            self.note((*line).to_owned());
        }
        Vec::new()
    }

    /// `/org`: asks to show the organisation `name`, or the next, on a client signed in to
    /// several.
    fn org(&mut self, name: Option<String>) -> Action {
        if self.orgs.len() < 2 {
            self.note("signed in to one organisation: zirc tui login signs in to another".into());
            return Action::Send(Vec::new());
        }
        Action::Org(name)
    }

    /// Completes who's being named before the cursor, as zirc-chat offers them
    /// ([`Chat::mentions`]): people, agent sessions by their token, apps. An `@` mention
    /// completes as the other apps' composers do; a bare word IRC's way, `alice: ` first.
    /// A `:shortcode` completes to the first emoji the other apps offer for it, and Tab again
    /// to the next.
    fn complete(&mut self) {
        let text = self.input.text().to_owned();
        let at = self.input.cursor_byte();
        if let Some(tabbed) = self.tabbed.take().and_then(|tabbed| tabbed.next(&text, at)) {
            self.input.replace(tabbed.wrote.0.clone(), tabbed.wrote.1);
            self.tabbed = Some(tabbed);
            return;
        }
        // `/join #na`: a channel to join, from those zirc-chat's `Chat::browse` lists.
        if let Some(typed) = joining(&text[..at]) {
            let typed = typed.trim_start_matches('#').to_lowercase();
            if let Some((name, _)) = self.chat.browse().into_iter().find(|(name, _)| {
                name.trim_start_matches('#')
                    .to_lowercase()
                    .starts_with(&typed)
            }) {
                self.input.replace_word_before_cursor(&format!("{name} "));
            }
            return;
        }
        match compose::completing(&text, at) {
            Some(Completing::Mention(typed)) => {
                if let Some(mention) = self.chat.mentions(typed, 1).first() {
                    let (completed, cursor) = complete_mention(&text, at, mention);
                    self.input.replace(completed, cursor);
                }
                return;
            }
            Some(Completing::Shortcode(query)) => {
                if let Some(tabbed) = Tabbed::new(query, &text, at) {
                    self.input.replace(tabbed.wrote.0.clone(), tabbed.wrote.1);
                    self.tabbed = Some(tabbed);
                }
                return;
            }
            None => {}
        }
        let (start, word) = self.input.word_before_cursor();
        let Some(mention) = self.chat.mentions(word, 1).into_iter().next() else {
            return;
        };
        let token = mention.token;
        let completed = if start == 0 {
            format!("{token}: ")
        } else {
            format!("{token} ")
        };
        self.input.replace_word_before_cursor(&completed);
    }

    fn scroll_up(&mut self) -> Vec<Line> {
        self.scroll_by(self.page().cast_signed())
    }

    fn scroll_down(&mut self) -> Vec<Line> {
        self.scroll_by(-self.page().cast_signed())
    }

    /// Half the scrollback, which Page Up and Page Down move by.
    fn page(&self) -> usize {
        (usize::from(self.view.1) / 2).max(1)
    }

    /// Scrolls `rows` towards older messages, or towards newer ones when negative, no
    /// further than the oldest, holding the view to what it then shows. Returns what to
    /// send: the page of history before, once it's within two screens of the top.
    fn scroll_by(&mut self, rows: isize) -> Vec<Line> {
        let height = usize::from(self.view.1).max(1);
        let (anchor, older) = {
            let laid = self.laid(usize::from(self.view.0));
            let Some(top) = self.view_of(&laid, height).top else {
                return Vec::new();
            };
            let top = laid.step(top, -rows);
            let at_end = laid.view(Some(top), height).at_end;
            (
                (!at_end).then(|| laid.anchor(top)),
                laid.wants_older(top, height),
            )
        };
        self.anchor(anchor, older)
    }

    /// Back to the newest of the conversation shown.
    fn back_to_end(&mut self) {
        self.scrolled.remove(&fold(&self.current().name));
    }
}

/// Browse channels, shown over the scrollback: what it lists is zirc-chat's `Chat::browse`
/// as it is when drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BrowsePanel;

/// Whether `key` leaves how every item is laid out as it was: typing, moving in what's
/// typed, picking and scrolling do; anything else might, as a command does.
fn lays_out_the_same(key: KeyEvent) -> bool {
    !key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        && matches!(
            key.code,
            KeyCode::Char(_)
                | KeyCode::Backspace
                | KeyCode::Delete
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::PageUp
                | KeyCode::PageDown
        )
}

/// The channel being typed after `/join` or `/j`, where `before` is the input up to the
/// cursor; `None` anywhere else.
fn joining(before: &str) -> Option<&str> {
    let rest = before
        .strip_prefix("/join ")
        .or_else(|| before.strip_prefix("/j "))?;
    // The last of `/join #a #b`.
    Some(rest.rsplit(' ').next().unwrap_or(rest))
}

fn quit(reason: Option<&str>) -> Line {
    Line::new("QUIT", [reason.unwrap_or("zirc-tui")])
}

/// A key's modifiers, `held`, as zirc-chat reads a press ([`keys::KeyPressed`]).
fn pressed(held: KeyModifiers) -> keys::KeyPressed {
    keys::KeyPressed {
        shift: held.contains(KeyModifiers::SHIFT),
        alt: held.contains(KeyModifiers::ALT),
        ctrl: held.contains(KeyModifiers::CONTROL),
        meta: held.contains(KeyModifiers::SUPER),
        ..keys::KeyPressed::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, TimeZone as _, Utc};
    use zirc_core::chat::entry_text;
    use zirc_core::chat::is_group;
    use zirc_irc::multiline::Limits;

    fn noon() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 30, 12, 0, 0).unwrap()
    }

    #[test]
    fn editor_picks_where_code_opens_and_keeps_it() {
        let mut app = app();
        app.chat
            .set_checkouts(vec![("#api".into(), "/src/api".into())]);
        typed(&mut app, "/editor forge");
        assert_eq!(app.prefs().editor.as_deref(), Some("forge"));
        // The forge isn't known, and no editor opens it: nothing links.
        assert!(app.chat.code_links("#api", "src/a.rs:4").is_empty());
        typed(&mut app, "/editor zed");
        assert_eq!(
            app.chat.code_links("#api", "src/a.rs:4")[0].1,
            "zed://file/src/api/src/a.rs:4"
        );
        typed(&mut app, "/editor automatic");
        assert_eq!(app.prefs().editor, None);
    }

    #[test]
    fn opens_files_named_in_messages_in_the_chosen_editor_in_zircds_checkout() {
        let mut app = app();
        app.use_prefs(Prefs {
            editor: Some("zed".into()),
            ..Prefs::default()
        });
        app.zircd_answered(Ok(Response::Repos {
            repos: vec![zirc_core::ipc::Repo {
                path: "/src/api".into(),
                scope: "#api".into(),
                sessions: Vec::new(),
                org: None,
                shared: Vec::new(),
            }],
        }));
        // A line in #agents about api counts as said in api's scope (Entry::place).
        let place = zirc_core::project::scope("api");
        assert_eq!(
            app.chat.code_links(&place, "broke at src/a.rs:4"),
            [(
                "src/a.rs:4".to_owned(),
                "zed://file/src/api/src/a.rs:4".to_owned()
            )]
        );
        assert_eq!(app.prefs().editor.as_deref(), Some("zed"));
    }

    fn app() -> App {
        let mut app = App::new("me", "irc.zirc.test", vec!["#extra".into()]);
        app.chat.clock = noon;
        app
    }

    fn link(echo: bool) -> Link {
        Link {
            nick: "me".into(),
            metadata: false,
            chathistory: true,
            echo,
            read_marker: false,
            trim: true,
            multiline: None,
        }
    }

    fn feed(app: &mut App, lines: &[&str]) -> Vec<String> {
        lines
            .iter()
            .flat_map(|line| app.handle(line.parse().unwrap()))
            .map(|line| line.to_string())
            .collect()
    }

    #[test]
    fn assigns_and_closes_a_guest_in_their_room_and_messages_one_there() {
        let mut app = app();
        let _ = app.connected(link(true));
        feed(
            &mut app,
            &[
                ":me!u@h JOIN #lobby",
                ":guest-7k2q!u@h JOIN #lobby * :Anna",
                ":guest-9z9z!u@h JOIN #lobby * :",
                ":dana!u@h JOIN #lobby dana :Dana",
                ":me!u@h JOIN #lobby/guest-7k2q",
                "@time=2026-10-30T11:00:00.000Z :guest-7k2q!u@h PRIVMSG #lobby/guest-7k2q :hello?",
            ],
        );
        // Outside a guest's room, there's nothing to assign.
        let _ = app.select(app.chat.position("#lobby").unwrap());
        assert!(sent(typed(&mut app, "/assign dana")).is_empty());
        let _ = app.select(app.chat.position("#lobby/guest-7k2q").unwrap());
        // Only to someone on staff.
        assert!(sent(typed(&mut app, "/assign zed")).is_empty());
        feed(
            &mut app,
            &[
                ":s 353 me = #lobby :@me dana guest-7k2q guest-9z9z",
                ":s 366 me #lobby :End",
            ],
        );
        assert_eq!(
            sent(typed(&mut app, "/assign dana")),
            [
                "@+zirc/support=assigned:dana TAGMSG #lobby/guest-7k2q",
                "@+zirc/support=joined:dana TAGMSG #lobby/guest-7k2q",
            ]
        );
        // Banning is for admins.
        assert!(sent(typed(&mut app, "/ban")).is_empty());
        assert_eq!(
            app.current().entries.last().map(entry_text).as_deref(),
            Some("only an admin can ban a guest")
        );
        assert_eq!(
            sent(typed(&mut app, "/close")),
            ["@+zirc/support=closed TAGMSG #lobby/guest-7k2q"]
        );
        // Still shown: closing resolves it, it doesn't leave the room.
        assert_eq!(app.current().name, "#lobby/guest-7k2q");
        // Messaging a guest says it in their room, which opening joins.
        let lines = sent(typed(&mut app, "/msg guest-9z9z hi"));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("JOIN #lobby/guest-9z9z")),
            "{lines:?}"
        );
        assert!(
            lines
                .last()
                .unwrap()
                .ends_with("PRIVMSG #lobby/guest-9z9z hi"),
            "{lines:?}"
        );
    }

    #[test]
    fn bans_a_guest_through_zirc_dev_as_an_admin_only() {
        let mut app = app();
        let _ = app.connected(link(true));
        feed(
            &mut app,
            &[
                ":me!u@h JOIN #lobby",
                ":guest-7k2q!u@h JOIN #lobby * :Anna",
                "@time=2026-10-30T11:00:00.000Z :guest-7k2q!u@h PRIVMSG me :hello?",
            ],
        );
        app.site = Some(Site {
            web: "https://zirc.dev".into(),
            slug: "acme".into(),
            home: None,
        });
        let _ = app.select(app.chat.position("guest-7k2q").unwrap());
        // A member is refused, and nothing's asked of zirc.dev.
        assert!(sent(typed(&mut app, "/ban")).is_empty());
        app.chat.set_admin(true);
        let Action::Guests { request, done } = typed(&mut app, "/ban") else {
            panic!("an admin bans through zirc.dev");
        };
        assert_eq!(
            (request.method, request.path.as_str()),
            ("POST", "/api/organisations/acme/guests/guest-7k2q/ban")
        );
        // What zirc.dev answers is said where we are.
        app.guests_answered(Ok(done));
        app.guests_answered(Err("not an admin of acme".into()));
        let said: Vec<String> = app
            .current()
            .entries
            .iter()
            .rev()
            .take(2)
            .map(entry_text)
            .collect();
        assert_eq!(
            said,
            [
                "zirc.dev: not an admin of acme",
                "Anna is banned from every public channel"
            ]
        );
    }

    #[test]
    fn bans_unbans_and_invites_a_guest_through_zirc_dev_as_an_admin() {
        let mut app = app();
        let _ = app.connected(link(true));
        feed(
            &mut app,
            &[
                ":me!u@h JOIN #lobby",
                ":guest-7k2q!u@h JOIN #lobby * :Anna",
                "@time=2026-10-30T11:00:00.000Z :guest-7k2q!u@h PRIVMSG me :hello?",
            ],
        );
        app.site = Some(Site {
            web: "https://zirc.dev".into(),
            slug: "acme".into(),
            home: None,
        });
        let _ = app.select(app.chat.position("guest-7k2q").unwrap());
        // A member who's no admin is refused all of it.
        for typed_in in ["/unban", "/invite"] {
            assert!(sent(typed(&mut app, typed_in)).is_empty(), "{typed_in}");
        }
        app.chat.set_role("admin");
        let asked = |action: Action| match action {
            Action::Guests { request, done } => (request.method, request.path, done),
            other => panic!("asked nothing of zirc.dev: {other:?}"),
        };
        assert_eq!(
            asked(typed(&mut app, "/invite")),
            (
                "POST",
                "/api/organisations/acme/guests/guest-7k2q/invite".to_owned(),
                "Anna is invited to join the organisation".to_owned()
            )
        );
        // From anywhere, by their nick.
        let _ = app.select(app.chat.position("#lobby").unwrap());
        assert_eq!(
            asked(typed(&mut app, "/unban guest-7k2q")),
            (
                "DELETE",
                "/api/organisations/acme/guests/guest-7k2q/ban".to_owned(),
                "Anna may come back to the public channels".to_owned()
            )
        );
        assert_eq!(
            asked(typed(&mut app, "/invite guest-7k2q")).1,
            "/api/organisations/acme/guests/guest-7k2q/invite"
        );
    }

    #[test]
    fn never_adds_a_guest_to_a_group() {
        let mut app = app();
        let _ = app.connected(link(true));
        feed(
            &mut app,
            &[
                ":me!u@h JOIN #lobby",
                ":guest-1a2b!u@h JOIN #lobby * :",
                ":me!u@h JOIN #dm-abc",
            ],
        );
        app.chat.set_role("admin");
        let _ = app.select(app.chat.position("#dm-abc").unwrap());
        assert!(is_group(&app.current().name));
        // Not with others, nor as an admin: a guest is invited to the organisation alone.
        assert!(sent(typed(&mut app, "/invite bob guest-1a2b")).is_empty());
        assert_eq!(
            app.current().entries.last().map(entry_text).as_deref(),
            Some(
                "a guest can't join a group: /invite <guest> alone invites them to the organisation"
            )
        );
        assert!(!sent(typed(&mut app, "/invite bob")).is_empty());
    }

    #[test]
    fn opens_our_account_page_on_zirc_dev() {
        let mut app = app();
        // Not one of zirc.dev's servers: said so, nothing opened.
        assert!(sent(typed(&mut app, "/profile")).is_empty());
        app.site = Some(Site {
            web: "https://zirc.dev/".into(),
            slug: "acme".into(),
            home: None,
        });
        assert_eq!(
            typed(&mut app, "/profile"),
            Action::Open("https://zirc.dev/account".into())
        );
    }

    fn typed(app: &mut App, text: &str) -> Action {
        for c in text.chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.key(KeyEvent::from(KeyCode::Enter))
    }

    fn sent(action: Action) -> Vec<String> {
        match action {
            Action::Send(lines) | Action::Quit(lines) => {
                lines.iter().map(ToString::to_string).collect()
            }
            Action::Zircd(request) => panic!("asked zircd {request:?}"),
            Action::Org(org) => panic!("asked for organisation {org:?}"),
            Action::Sessions => panic!("asked for the sessions"),
            Action::Guests { request, .. } => panic!("asked zirc.dev {request:?}"),
            Action::Open(url) => panic!("opened {url}"),
            Action::Press(press) => panic!("pressed {press:?}"),
        }
    }

    #[test]
    fn nick_points_to_zirc_dev_when_signed_in_there() {
        let mut app = app();
        app.connected(link(true));
        // Without zirc.dev, the plain IRC command.
        assert_eq!(sent(typed(&mut app, "/nick bob")), ["NICK bob"]);
        app.site = Some(Site {
            web: "https://zirc.dev".into(),
            slug: "acme".into(),
            home: None,
        });
        // Signed in there, it's the account's, changed on its page, which opens.
        assert_eq!(
            typed(&mut app, "/nick Lovelace"),
            Action::Open("https://zirc.dev/account".into())
        );
    }

    #[test]
    fn records_a_decision_from_our_own_account_in_decisions_and_here() {
        let mut app = app();
        app.connected(link(true));
        feed(&mut app, &[":me!u@h JOIN #api"]);
        let sent = sent(typed(&mut app, "/decide cloud spend: yes, add the node"));
        assert_eq!(sent[0], "JOIN #decisions");
        let to = |target: &str| {
            sent.iter()
                .filter(|line| line.starts_with(&format!("PRIVMSG {target} :\u{1}ACTION ZIRC ")))
                .count()
        };
        assert_eq!((to("#decisions"), to("#api")), (1, 1), "{sent:?}");
        let body = sent[1].split_once(" :").unwrap().1;
        let message = zirc_proto::decode(body).unwrap().unwrap();
        let decision = zirc_proto::decision::Decision::of(&message.event).unwrap();
        assert_eq!(
            (
                decision.by.as_str(),
                decision.about.as_str(),
                decision.session
            ),
            ("me", "cloud spend", None)
        );
    }

    #[test]
    fn sets_who_may_query_this_machines_agents_through_zircd() {
        let mut app = app();
        let ask = |action: Action| match action {
            Action::Zircd(request) => request,
            other => panic!("didn't ask zircd: {other:?}"),
        };
        assert_eq!(
            ask(typed(&mut app, "/queries")),
            Request::Queries { set: None }
        );
        assert_eq!(
            ask(typed(&mut app, "/queries off")),
            Request::Queries {
                set: Some(Queries::Off)
            }
        );
        // Anyone only after the warning, with yes.
        assert!(sent(typed(&mut app, "/queries anyone")).is_empty());
        let said = texts(app.current());
        assert!(
            said.iter().any(|line| line.contains("prompt injection")),
            "{said:?}"
        );
        assert_eq!(
            ask(typed(&mut app, "/queries anyone yes")),
            Request::Queries {
                set: Some(Queries::Anyone)
            }
        );
        assert!(sent(typed(&mut app, "/queries everyone")).is_empty());

        app.zircd_answered(Ok(Response::Queries {
            queries: Queries::Mine,
            colleagues_kept_out: false,
        }));
        app.zircd_answered(Err("zircd isn't running".into()));
        let said = texts(app.current());
        assert!(
            said.iter()
                .any(|line| line.ends_with("queries to this machine's agents: mine"))
        );
        assert!(said.last().unwrap().ends_with("zircd: zircd isn't running"));
    }

    fn texts(conversation: &Conversation) -> Vec<String> {
        conversation.entries.iter().map(entry_text).collect()
    }

    #[test]
    fn sends_what_is_typed() {
        let mut app = app();
        app.connected(link(true));
        feed(&mut app, &[":me!u@h JOIN #api"]);
        assert_eq!(app.current().name, "#api");
        assert_eq!(
            sent(typed(&mut app, "hello")),
            ["@+zirc.dev/client=tui PRIVMSG #api hello"]
        );
        assert_eq!(
            sent(typed(&mut app, "/me waves")),
            ["@+zirc.dev/client=tui PRIVMSG #api :\x01ACTION waves\x01"]
        );
        assert_eq!(
            sent(typed(&mut app, "/msg bob hi")),
            [
                "CHATHISTORY LATEST bob * 200",
                "@+zirc.dev/client=tui PRIVMSG bob hi"
            ]
        );
        assert_eq!(app.current().name, "bob");
        assert_eq!(sent(typed(&mut app, "/join #web")), ["JOIN #web"]);
        feed(&mut app, &[":me!u@h JOIN #web"]);
        assert_eq!(app.current().name, "#web");
        assert!(sent(typed(&mut app, "/part")).is_empty());
        assert_eq!(
            press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE),
            ["PART #web"]
        );
        feed(&mut app, &[":me!u@h PART #web"]);
        // A channel left gives way to the one before it.
        assert_eq!(app.current().name, "#api");
        assert!(matches!(typed(&mut app, "/quit bye"), Action::Quit(_)));
        // With echo, what we say shows when the server sends it back.
        assert!(app.conversations().iter().all(|c| {
            c.entries.iter().all(|e| {
                matches!(
                    e.kind,
                    zirc_core::chat::EntryKind::Info(_) | zirc_core::chat::EntryKind::Part(_)
                ) || e.from != "me"
            })
        }));
    }

    #[test]
    fn enter_with_any_modifier_is_a_new_line_and_plain_enter_sends() {
        let mut app = app();
        app.connected(link(true));
        feed(&mut app, &[":me!u@h JOIN #api"]);
        let _ = app.select(0);
        app.input.insert('a');
        for held in [
            KeyModifiers::SHIFT,
            KeyModifiers::ALT,
            KeyModifiers::CONTROL,
            KeyModifiers::SUPER,
        ] {
            app.key(KeyEvent::new(KeyCode::Enter, held));
        }
        assert_eq!(app.input.text(), "a\n\n\n\n");
        assert!(matches!(
            app.key(KeyEvent::from(KeyCode::Enter)),
            Action::Send(_)
        ));
        assert!(app.input.text().is_empty());
    }

    #[test]
    fn typing_goes_on_in_the_composer_as_conversations_change_and_messages_come() {
        let mut app = app();
        app.connected(link(true));
        feed(&mut app, &[":me!u@h JOIN #api", ":me!u@h JOIN #web"]);
        let _ = app.select(0);
        app.key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        for c in "typing".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        feed(
            &mut app,
            &[":bob!u@h PRIVMSG #api :hi", ":bob!u@h PRIVMSG #web :hi"],
        );
        for c in " on".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        assert_eq!(app.input.text(), "typing on");
    }

    #[test]
    fn a_draft_stays_with_its_conversation_and_never_follows_into_another() {
        let mut app = app();
        app.connected(link(true));
        feed(&mut app, &[":me!u@h JOIN #api", ":me!u@h JOIN #web"]);
        let api = app
            .conversations()
            .iter()
            .position(|c| c.name == "#api")
            .unwrap();
        let web = app
            .conversations()
            .iter()
            .position(|c| c.name == "#web")
            .unwrap();
        let _ = app.select(api);
        app.input.insert_str("half a thought");
        let _ = app.select(web);
        assert_eq!(app.input.text(), "");
        app.input.insert_str("over here");
        let _ = app.select(api);
        assert_eq!(app.input.text(), "half a thought");
        let _ = app.select(web);
        assert_eq!(app.input.text(), "over here");
    }

    #[test]
    fn goes_to_conversations_by_key() {
        let mut app = app();
        app.connected(link(true));
        feed(&mut app, &[":me!u@h JOIN #api"]);
        let _ = app.select(0);
        feed(
            &mut app,
            &[
                ":alice!u@h PRIVMSG #api :me: look",
                ":bob!u@h PRIVMSG me :psst",
            ],
        );
        app.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::ALT));
        assert_eq!(app.current().name, "#api");
        app.key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        assert_eq!(app.current().name, "bob");
        app.key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        assert_eq!(app.current().name, "#api", "round the ends, to the top");
        app.key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        assert_eq!(app.current().name, "bob", "and on from there");
        app.key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::ALT));
        assert_eq!(app.current().name, "bob");
        app.key(KeyEvent::new(KeyCode::Char('9'), KeyModifiers::ALT));
        assert_eq!(app.current().name, "bob", "there's no ninth");
        let _ = app.select(0);
        app.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert_eq!(app.current().name, "bob", "from the server's to the last");
    }

    #[test]
    fn sends_long_text_as_one_multiline_message() {
        let mut app = app();
        app.connected(Link {
            multiline: Some(Limits {
                max_bytes: 4096,
                max_lines: 100,
            }),
            ..link(true)
        });
        feed(&mut app, &[":me!u@h JOIN #api"]);
        let long = "word ".repeat(100);
        let lines = sent(typed(&mut app, long.trim()));
        assert!(
            lines[0].starts_with("@+zirc.dev/client=tui BATCH +tui1 draft/multiline #api"),
            "{lines:?}"
        );
        assert_eq!(lines.last().unwrap(), "BATCH -tui1");
    }

    #[test]
    fn explains_mistakes_where_they_were_made() {
        let mut app = app();
        app.connected(link(true));
        assert!(sent(typed(&mut app, "hello")).is_empty());
        assert!(
            texts(app.current())
                .last()
                .unwrap()
                .contains("isn't a conversation")
        );
        assert!(sent(typed(&mut app, "/topic x")).is_empty());
        assert!(
            texts(app.current())
                .last()
                .unwrap()
                .contains("for channels")
        );
    }

    #[test]
    fn completes_nicks() {
        let mut app = app();
        app.connected(link(true));
        feed(
            &mut app,
            &[
                ":me!u@h JOIN #api",
                ":s 353 me = #api :me Alice bob",
                ":s 366 me #api :End",
            ],
        );
        for c in "al".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.input.text(), "Alice: ");
        for c in "and b".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.input.text(), "Alice: and bob ");
    }

    #[test]
    fn completes_shortcodes_cycling_on_tab_and_writes_one_closed_as_its_typed() {
        let mut app = app();
        app.connected(link(true));
        let keys = |app: &mut App, text: &str| {
            for c in text.chars() {
                app.key(KeyEvent::from(KeyCode::Char(c)));
            }
        };
        keys(&mut app, "yay :tad");
        app.key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.input.text(), "yay 🎉 ");
        // Tab again writes the next emoji the other apps offer for `:tad`, then comes round.
        let offered = emoji::suggest("tad", picker::NEWEST, emoji::OFFERED);
        if let Some(second) = offered.get(1) {
            app.key(KeyEvent::from(KeyCode::Tab));
            assert_eq!(app.input.text(), format!("yay {} ", second.emoji));
        }
        for _ in 1..offered.len() {
            app.key(KeyEvent::from(KeyCode::Tab));
        }
        assert_eq!(app.input.text(), "yay 🎉 ");
        keys(&mut app, ":joy:");
        assert_eq!(app.input.text(), "yay 🎉 😂");
    }

    #[test]
    fn completes_an_agent_by_its_session_as_the_other_apps_do() {
        let mut app = app();
        app.connected(link(true));
        let mut post = zirc_proto::Message::new(
            "cc-598aa1",
            zirc_proto::Event::Text {
                body: "on it".into(),
            },
        );
        post.name = Some("zirc-chat".into());
        feed(
            &mut app,
            &[
                ":me!u@h JOIN #api",
                &format!(
                    ":mattias-mbp!u@machine/mattias PRIVMSG #api :{}",
                    zirc_proto::encode(&post).unwrap()
                ),
            ],
        );
        let _ = app.select(app.chat.position("#api").unwrap());
        // By its name, to its session's token.
        for c in "thanks @zir".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.input.text(), "thanks @cc-598aa1 ");
    }

    #[test]
    fn shows_only_people_in_one_conversation_and_remembers_which() {
        let mut app = app();
        app.connected(link(true));
        feed(&mut app, &[":me!u@h JOIN #api", ":me!u@h JOIN #web"]);
        let _ = app.select(app.chat.position("#api").unwrap());
        typed(&mut app, "/people");
        assert!(app.people_only());
        let _ = app.select(app.chat.position("#web").unwrap());
        assert!(!app.people_only(), "only where it was asked for");
        assert_eq!(app.take_prefs().unwrap().people_only, ["#api"]);
    }

    #[test]
    fn show_picks_what_a_conversation_shows_and_keeps_it() {
        let mut app = app();
        app.connected(link(true));
        feed(&mut app, &[":me!u@h JOIN #api", ":me!u@h JOIN #web"]);
        let _ = app.select(app.chat.position("#api").unwrap());
        typed(&mut app, "/show");
        let said = texts(app.current()).last().unwrap().clone();
        assert!(
            said.contains("errors · warnings · [info] · everything"),
            "{said}"
        );
        assert_eq!(app.showing(), None);
        typed(&mut app, "/show Warnings");
        assert_eq!(app.chat.threshold("#api"), zirc_proto::Level::Warn);
        assert_eq!(app.showing().as_deref(), Some("warnings and up"));
        let _ = app.select(app.chat.position("#web").unwrap());
        assert_eq!(app.showing(), None, "only where it was asked for");
        let prefs = app.take_prefs().unwrap();
        assert_eq!(
            prefs.thresholds.get("#api").map(String::as_str),
            Some("warn")
        );
        // Kept, it comes back.
        let mut again = super::tests::app();
        again.use_prefs(prefs);
        assert_eq!(again.chat.threshold("#api"), zirc_proto::Level::Warn);
        typed(&mut app, "/show loud");
        assert!(
            texts(app.current())
                .last()
                .unwrap()
                .contains("/show errors")
        );
    }

    #[test]
    fn lists_starred_channels_first_then_the_rest_a_to_z() {
        let mut app = app();
        app.connected(link(true));
        feed(
            &mut app,
            &[
                ":me!u@h JOIN #zoo",
                ":me!u@h JOIN #api",
                ":me!u@h JOIN #web",
                ":me!u@h JOIN #mid",
                // Said last, which doesn't move it: the list goes by name.
                ":bob!u@h PRIVMSG #zoo :hi",
            ],
        );
        app.chat.set_stars(["#web".to_owned()]);
        let listed: Vec<String> = app
            .sidebar()
            .into_iter()
            .filter_map(|row| match row {
                Side::Heading { label, .. } => Some(label.to_owned()),
                Side::Row {
                    label, depth: 0, ..
                } => Some(label),
                _ => None,
            })
            .take_while(|label| label != Shelf::Direct.label())
            .collect();
        let (starred, channels) = (Shelf::Starred.label(), Shelf::Channels.label());
        assert_eq!(listed, [starred, "#web", channels, "#api", "#mid", "#zoo"]);
    }

    #[test]
    fn lists_guests_rooms_under_their_channel_for_an_admin_and_joins_one_clicked() {
        let mut app = app();
        app.connected(link(true));
        feed(&mut app, &[":me!u@h JOIN #api"]);
        app.chat.set_role("admin");
        let kept = |nick: String| zirc_core::chat::KeptGuest {
            rooms: Vec::new(),
            name: (nick == "guest-21042").then(|| "Sam".to_owned()),
            // Sam's connected, so waiting on us; the rest fold away.
            connected: nick == "guest-21042",
            nick,
            channels: vec!["#api".into()],
        };
        app.chat.set_kept_guests(vec![kept("guest-21042".into())]);
        let labels = |app: &App| -> Vec<(String, usize, Option<Target>)> {
            app.sidebar()
                .into_iter()
                .filter_map(|row| match row {
                    Side::Row {
                        label,
                        depth: 1,
                        target,
                        ..
                    } => Some((label, 1, target)),
                    _ => None,
                })
                .collect()
        };
        let click = MouseEventKind::Down(MouseButton::Left);
        let fold = Target::GuestRooms("#api".into());
        // Folded under #api's own row by default, which a mark at its end opens.
        assert!(labels(&app).is_empty());
        let marked = app.sidebar().into_iter().any(|row| {
            matches!(row, Side::Row { fold: Some((ref channel, false)), .. } if channel == "#api")
        });
        assert!(marked);
        app.pointer(click, Some(fold.clone()));
        let room = Target::Open("#api/guest-21042".into());
        assert_eq!(labels(&app), [("Sam".to_owned(), 1, Some(room.clone()))]);
        assert_eq!(
            sent(app.pointer(click, Some(room))),
            ["JOIN #api/guest-21042"]
        );

        // However many, they fold and open together.
        app.chat.set_kept_guests(
            (0..7)
                .map(|n| kept(format!("guest-{n}{n}{n}{n}")))
                .collect(),
        );
        assert_eq!(labels(&app).len(), 7);
        app.pointer(click, Some(fold));
        assert!(labels(&app).is_empty());
    }

    #[test]
    fn clicks_show_conversations_and_who_people_are() {
        let mut app = app();
        app.connected(link(true));
        feed(
            &mut app,
            &[
                ":me!u@h JOIN #api",
                ":s 353 me = #api :me alice",
                ":s 366 me #api :End",
            ],
        );
        let click = MouseEventKind::Down(MouseButton::Left);
        let _ = app.select(0);
        let api = app.chat.position("#api").unwrap();
        assert!(sent(app.pointer(click, Some(Target::Conversation(api)))).is_empty());
        assert_eq!(app.current().name, "#api");
        // Somewhere not in the list does nothing.
        app.pointer(click, Some(Target::Conversation(99)));
        assert_eq!(app.current().name, "#api");

        // Someone in the member list: who they are, asking the server for the rest.
        let lines = sent(app.pointer(click, Some(Target::Member("alice".into()))));
        assert_eq!(lines, ["WHOIS alice"]);
        assert_eq!(app.profile(), Some("alice"));
        assert_eq!(app.current().name, "#api");
        // Esc closes it; Enter talks to them.
        assert!(sent(app.key(KeyEvent::from(KeyCode::Esc))).is_empty());
        assert_eq!(app.profile(), None);
        app.pointer(click, Some(Target::Member("alice".into())));
        let lines = sent(app.key(KeyEvent::from(KeyCode::Enter)));
        assert_eq!(app.current().name, "alice");
        assert_eq!(lines, ["CHATHISTORY LATEST alice * 200"]);
        assert_eq!(app.profile(), None);
        // Ourselves too, where Enter just closes it; and only the left button.
        app.pointer(click, Some(Target::Member("ME".into())));
        assert_eq!(app.profile(), Some("ME"));
        assert!(sent(app.key(KeyEvent::from(KeyCode::Enter))).is_empty());
        assert_eq!(app.profile(), None);
        app.pointer(
            MouseEventKind::Down(MouseButton::Right),
            Some(Target::Conversation(api)),
        );
        assert_eq!(app.current().name, "alice");
    }

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Vec<String> {
        sent(app.key(KeyEvent::new(code, modifiers)))
    }

    #[test]
    fn reacts_to_the_newest_or_the_message_picked() {
        let mut app = app();
        app.connected(link(true));
        feed(
            &mut app,
            &[
                ":me!u@h JOIN #api",
                "@msgid=m1 :alice!u@h PRIVMSG #api :shipped it",
                "@msgid=m2 :bob!u@h PRIVMSG #api :nice",
            ],
        );
        let react =
            |emoji: &str, id: &str| format!("@+draft/react={emoji};+draft/reply={id} TAGMSG #api");
        // By name, to the newest.
        assert_eq!(sent(typed(&mut app, "/react tada")), [react("🎉", "m2")]);
        // Shift-↑ picks the newest, then the one before; Shift-↓ past the newest lets go.
        press(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
        assert_eq!(app.picked(), Some("m2"));
        press(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
        assert_eq!(app.picked(), Some("m1"));
        press(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
        assert_eq!(app.picked(), Some("m1"), "nothing older");
        assert_eq!(sent(typed(&mut app, "/react :+1:")), [react("👍", "m1")]);
        // The picker: what was used lately first.
        press(&mut app, KeyCode::Char('r'), KeyModifiers::ALT);
        assert_eq!(app.picker().map(|p| p.id.as_str()), Some("m1"));
        assert_eq!(app.picker().unwrap().chosen(&app.recent), Some("👍"));
        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE),
            [react("🎉", "m1")]
        );
        assert!(app.picker().is_none());
        // Typing in it searches rather than going to the input line; Esc closes it.
        assert!(sent(typed(&mut app, "/react")).is_empty());
        for c in "rocket".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        assert_eq!(app.input.text(), "");
        assert_eq!(app.picker().unwrap().chosen(&app.recent), Some("🚀"));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.picker().is_none());
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.picked(), None);

        // Clicking a reaction we gave takes it back; clicking a message picks it.
        feed(
            &mut app,
            &["@+draft/react=🎉;+draft/reply=m1 :me!u@h TAGMSG #api"],
        );
        let click = MouseEventKind::Down(MouseButton::Left);
        let lines = sent(app.pointer(
            click,
            Some(Target::Reaction {
                id: "m1".into(),
                emoji: "🎉".into(),
            }),
        ));
        assert_eq!(lines, ["@+draft/unreact=🎉;+draft/reply=m1 TAGMSG #api"]);
        app.pointer(click, Some(Target::Message("m2".into())));
        assert_eq!(app.picked(), Some("m2"));

        // What can't be done is said.
        typed(&mut app, "/react zzzzqx");
        assert!(
            texts(app.current())
                .last()
                .unwrap()
                .contains("no emoji called zzzzqx")
        );
        let _ = app.select(0);
        typed(&mut app, "/react tada");
        assert!(
            texts(app.current())
                .last()
                .unwrap()
                .contains("nothing here to react to")
        );
    }

    #[test]
    fn leaves_a_channel_only_once_asked() {
        let mut app = app();
        app.connected(link(true));
        feed(
            &mut app,
            &[
                ":me!u@h JOIN #api",
                ":bob!u@h JOIN #api",
                ":me!u@h JOIN #web",
            ],
        );
        let _ = app.select(app.chat.position("#api").unwrap());
        // Asked first, in zirc-chat's words; anything but y keeps it, and isn't typed.
        assert!(sent(typed(&mut app, "/part")).is_empty());
        assert_eq!(app.leaving(), Some(app.chat.leave_prompt("#api")));
        assert!(press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE).is_empty());
        assert_eq!((app.leaving(), app.input.text()), (None, ""));
        // Named, or with a reason, it still asks.
        assert!(sent(typed(&mut app, "/leave #web done for today")).is_empty());
        assert_eq!(
            press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE),
            ["PART #web :done for today"]
        );
        assert!(app.leaving().is_none());
    }

    #[test]
    fn browses_the_channels_were_not_in_and_completes_one_to_join() {
        let mut app = app();
        app.connected(link(true));
        feed(&mut app, &[":me!u@h JOIN #api"]);
        let _ = app.select(app.chat.position("#api").unwrap());
        feed(
            &mut app,
            &[
                ":s 322 me #api 2 :",
                ":s 322 me #api/login 1 :",
                ":s 322 me #api/checkout 3 :",
                ":s 323 me :End of LIST",
            ],
        );
        typed(&mut app, "/browse");
        assert!(app.browsing());
        // It stays while a /join is typed, and Tab completes from it.
        for c in "/join #api/l".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.input.text(), "/join #api/login ");
        assert!(app.browsing());
        // Sending closes it, as Esc does.
        assert_eq!(
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE),
            ["JOIN #api/login"]
        );
        assert!(!app.browsing());
        typed(&mut app, "/browse");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.browsing());
    }

    #[test]
    fn edits_our_message_picked_with_e() {
        let mut app = app();
        app.connected(link(true));
        feed(
            &mut app,
            &[
                ":me!u@h JOIN #api",
                "@msgid=m1 :me!u@h PRIVMSG #api :teh fix",
                "@msgid=m2 :bob!u@h PRIVMSG #api :nice",
            ],
        );
        // Someone else's isn't ours to correct.
        press(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
        assert_eq!(app.picked(), Some("m2"));
        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE);
        assert!(!app.editing());
        // Ours goes in the input; Esc stops, emptying it.
        press(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE);
        assert!(app.editing());
        assert_eq!(app.input.text(), "teh fix");
        // The status line names it, as every app's Editing bar does.
        assert_eq!(
            app.editing_after(),
            Some(zirc_core::chat::editing_after("teh fix"))
        );
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!((app.editing(), app.input.text()), (false, ""));
        // Sent, it carries what it corrects.
        press(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
        press(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
        press(&mut app, KeyCode::Char('e'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Home, KeyModifiers::NONE);
        for _ in 0..3 {
            press(&mut app, KeyCode::Delete, KeyModifiers::NONE);
        }
        for c in "the".chars() {
            app.key(KeyEvent::from(KeyCode::Char(c)));
        }
        assert_eq!(
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE),
            ["@+zirc.dev/client=tui;+draft/edit=m1 PRIVMSG #api :the fix"]
        );
        assert!(!app.editing());
        feed(
            &mut app,
            &["@msgid=m3;+draft/edit=m1 :me!u@h PRIVMSG #api :the fix"],
        );
        let api = app.chat.conversation("#api").unwrap();
        assert!(api.entries.iter().any(|entry| entry.edited));
    }

    #[test]
    fn trims_the_message_picked_once_confirmed() {
        let mut app = app();
        app.connected(link(true));
        feed(
            &mut app,
            &[
                ":me!u@h JOIN #api",
                "@msgid=m1 :me!u@h PRIVMSG #api :the key is hunter2",
                "@msgid=m2 :bob!u@h PRIVMSG #api :nice",
            ],
        );
        press(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
        press(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
        assert_eq!(app.picked(), Some("m1"));
        // Asked first, saying which; anything but y keeps it.
        assert!(sent(typed(&mut app, "/trim")).is_empty());
        let trimming = app.trimming().unwrap();
        assert_eq!(
            (trimming.id.as_str(), trimming.text.as_str()),
            ("m1", "<me> the key is hunter2")
        );
        assert!(press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE).is_empty());
        assert!(app.trimming().is_none());
        assert_eq!(app.input.text(), "", "the key isn't typed");
        typed(&mut app, "/trim a secret");
        assert_eq!(
            press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE),
            ["REDACT #api m1 :a secret"]
        );
        assert_eq!((app.trimming(), app.picked()), (None, None));
        // Once the server says, it's gone; if it won't, it says why.
        feed(&mut app, &[":me!u@h REDACT #api m1 :a secret"]);
        assert_eq!(texts(app.current()), ["<bob> nice"]);
        typed(&mut app, "/trim");
        assert_eq!(
            app.trimming().unwrap().id,
            "m2",
            "the newest, with none picked"
        );
        press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
        feed(
            &mut app,
            &[":s FAIL REDACT REDACT_FORBIDDEN #api m2 :You are not authorized"],
        );
        assert!(
            texts(app.current())
                .last()
                .unwrap()
                .contains("couldn't trim the message: You are not authorized")
        );

        // A server that doesn't allow it says so up front.
        let mut plain = self::app();
        plain.connected(Link {
            trim: false,
            ..link(true)
        });
        feed(
            &mut plain,
            &[":me!u@h JOIN #api", "@msgid=m1 :me!u@h PRIVMSG #api :hi"],
        );
        typed(&mut plain, "/trim");
        assert!(plain.trimming().is_none());
        assert!(
            texts(plain.current())
                .last()
                .unwrap()
                .contains("doesn't let messages be trimmed")
        );
    }

    #[test]
    fn the_wheel_scrolls_no_further_than_the_oldest() {
        let mut app = app();
        app.connected(link(true));
        feed(&mut app, &[":me!u@h JOIN #api"]);
        for i in 0..30 {
            feed(&mut app, &[&format!(":alice!u@h PRIVMSG #api :line {i}")]);
        }
        app.view = (80, 10);
        app.pointer(MouseEventKind::ScrollUp, Some(Target::Scrollback));
        assert_eq!(app.scroll(), 3);
        for _ in 0..20 {
            app.pointer(MouseEventKind::ScrollUp, None);
        }
        let oldest = app.scroll();
        assert!(oldest > 3, "{oldest}");
        app.pointer(MouseEventKind::ScrollUp, None);
        assert_eq!(app.scroll(), oldest, "no further than the oldest");
        app.pointer(MouseEventKind::ScrollDown, None);
        assert_eq!(app.scroll(), oldest - 3);
        for _ in 0..10 {
            app.pointer(MouseEventKind::ScrollDown, None);
        }
        assert_eq!(app.scroll(), 0);
    }

    #[test]
    fn scrolls_each_conversation_on_its_own() {
        let mut app = app();
        app.connected(link(true));
        feed(&mut app, &[":me!u@h JOIN #api"]);
        for i in 0..50 {
            feed(&mut app, &[&format!(":alice!u@h PRIVMSG #api :line {i}")]);
        }
        app.view = (80, 10);
        app.key(KeyEvent::from(KeyCode::PageUp));
        assert_eq!(app.scroll(), 5);
        let _ = app.select(0);
        assert_eq!(app.scroll(), 0);
        let _ = app.select(1);
        app.key(KeyEvent::from(KeyCode::PageDown));
        assert_eq!(app.scroll(), 0);
    }

    #[test]
    fn switches_organisation_only_when_signed_in_to_several() {
        let mut app = app();
        // With one, Alt-O does nothing and /org says how to add another.
        assert!(sent(app.key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::ALT))).is_empty());
        typed(&mut app, "/org");
        assert!(
            texts(app.current())
                .iter()
                .any(|line| line.contains("zirc tui login"))
        );
        typed(&mut app, "/help");
        assert!(
            !texts(app.current())
                .iter()
                .any(|line| line.contains("/org"))
        );

        app.orgs = vec![
            ("acme".into(), Unread::default()),
            ("globex".into(), Unread::default()),
        ];
        app.org = Some("acme".into());
        assert_eq!(
            app.key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::ALT)),
            Action::Org(None)
        );
        assert_eq!(
            typed(&mut app, "/org globex"),
            Action::Org(Some("globex".into()))
        );
        typed(&mut app, "/help");
        assert!(
            texts(app.current())
                .iter()
                .any(|line| line.contains("/org [name]"))
        );
    }
}
