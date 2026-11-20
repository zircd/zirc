//! An IRC chat session as zirc's apps show it, without any UI: which conversations there
//! are, what was said in each, who is in them, what's unread and what mentions us, and who
//! is changing which files. [`Chat::receive`] takes each line from the server and the
//! methods that act for the person return the lines to send; nothing here touches the
//! network. The terminal client, the desktop app and the Android app each draw it their
//! own way.
//!
//! It asks the server for its channels and joins them, fetches history with
//! `draft/chathistory` (and the private conversations that have some), reassembles
//! `draft/multiline` messages, and keeps history and live messages in time order without
//! repeats. A message trimmed from the server's history ([`crate::trim`]) is dropped, as
//! anyone trims it.
//!
//! The person talks to each agent session on their own machines in a conversation of its
//! own, named `<machine>/<session>` ([`agent`]): what they say there goes to the machine as
//! `<session> …`, which its zircd hands to that session alone, and what the session
//! answers with `zirc dm` carries its name, which files it there.

mod activity;
mod agent_activity;
mod agent_card;
mod avatar;
mod block;
mod body;
mod both;
mod channel_rows;
mod channels;
pub use channels::{BrowseRow, JoinOffer, browse_rows, create_label, join_label};
mod closing;
mod code;
mod code_ref;
mod connection;
mod conversation;
mod copied;
mod correcting;
mod describe;
mod details;
mod drafts;
mod face;
pub mod file;
mod finder;
mod finding;
mod flat;
mod forge;
mod group;
mod guest_rooms;
mod guests;
mod header_line;
mod history;
mod incoming;
mod layout;
/// A conversation's lines as every app's records carry them. [`lines::Line`] isn't
/// re-exported here, where `Line` is an IRC line.
pub mod lines;
mod list;
mod live;
mod members;
mod more;
mod names;
mod navigate;
mod new_message;
mod notify;
mod org;
mod people;
mod plain;
mod preview;
mod profile;
pub mod push;
mod rich;
mod roster;
mod row_menu;
mod run;
mod scroll;
mod sealing;
mod search;
mod seek;
mod sending;
mod settings_list;
mod sidebar;
mod speaker;
mod split;
mod status;
mod thread;
mod titles;
mod typing;
mod unread;
mod view;
mod words;
mod working;

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Utc};
use zirc_line::Line;
use zirc_line::multiline::{self, Batches, Limits};
use zirc_proto::{Event, Message};

pub use activity::{ActivityKind, ActivityRow, InboxButton, MENTIONED, REACTED, REPLIED, UNREAD};
pub use agent_activity::{
    AGENTS_GLYPH, AGENTS_LABEL, ActivityEntry, AgentHeader, AgentIntro, ReadOnlyNote,
    activity_conversation,
};
pub use agent_card::{AgentCard, MESSAGE, OPEN_SESSION};
pub use avatar::{initials, nick_degrees};
pub use block::{
    BLOCKING_EXPLAINED, REPORT_EXPLAINED, REPORT_GONE, REPORT_NO_REASON, REPORT_NOT_REPORTABLE,
    REPORT_SELF, Reported,
};
pub use body::{Body, formatted, keep_formatting, read, split_for_line, strip_formatting};
pub use both::{
    BOTH_CHANGING_KIND, BOTH_CHANGING_SHOWN, BothChanger, BothChanging, BothChangingCard,
    BothChangingLine, both_changing_files_label, both_changing_words,
};
pub use channel_rows::ChannelRow;
pub use closing::Closed;
pub use code::{Role, Token, highlight};
pub use code_ref::{
    CodeAction, CodePreview, CodeRef, EDITOR_AUTOMATIC, EDITOR_FORGE, Editor, EditorOption,
    NO_PREVIEW, OPEN_CODE_IN, PREVIEW_AROUND, chosen_editor, code_actions, code_preview,
    editor_options, file_page, raw_file,
};
pub use conversation::{
    Attendance, CLIENT_TAG, COMMITS_AND_PUSHES_SHOW_AT, Conversation, Entry, EntryKind, Kind,
    Member, PREFIXES, QUOTE_CHARS, Quote, Reaction, SESSION_MARK, agent, default_threshold, fold,
    for_agent, is_channel, target, to_agent,
};
pub use copied::{CopiedMessage, copied};
pub use correcting::{
    EDIT_LABEL, EDITED, EDITING, STOP_EDITING, TOO_LATE_TO_EDIT, editable, editing_after,
};
pub use describe::{
    Described, EventKind, RUN_GAP, copy_text, describe, entry_text, first_link, notice_text, sender,
};
pub use details::{DATA, Detail, Details, MESSAGE_DETAILS, details};
pub use face::{Face, LineSpeaker};
pub use finder::{
    Elsewhere, GROUP_HITS, GROUPS_SHOWN, MORE_RESULTS, PAGE, RECENT_SEARCHES, RecentSearches,
    SEARCH_EVERYWHERE, SEARCHING, ScreenChip, SearchEmpty, SearchFilter, SearchGroup, SearchLimits,
    SearchScreen, SearchSegments, ServerSearch, SortChoice, absorb as absorb_search,
    compose as compose_search, segment_scope, segments as search_segments, sort_a11y, sort_name,
};
pub use finding::{Finding, SEARCH_SETTLE_MS};
pub use flat::{
    CodeLine, CodeToken, Highlighter, RichBlock, RichCell, RichItem, RichRow, RichSpan,
    RichSpanKind, RichStub, TASK_MARKERS, highlighted_lines,
};
pub use forge::{branch_link, commit_link, link_blocks, link_words};
pub use group::{GROUP_FACES, GROUP_PREFIX, is_group};
pub use guest_rooms::{
    GuestRoom, GuestRooms, KEPT_GUESTS_EVERY, KEPT_GUESTS_PATH, KeptGuest, is_guest_room,
    kept_guests_path, parse_kept_guests,
};
pub use guests::{
    COMPOSER_NOTE, GATEWAY_REALNAME, GUEST_PILL, GuestAction, GuestHeader, GuestRequest, OUTSIDE,
    SIGNED_IN_KEY, assigned_label, guest_label, support_text,
};
pub use header_line::{HeaderLine, HeaderLineKind};
pub use layout::{ClockAt, Heading, row_heading};
pub use lines::{
    ANIMATION_PLAYS, CardAction, CardButtons, FileShape, LineActions, LineFile, LineKind, LineMore,
    LineQuote, LineReaction, MEDIA_HEIGHT, MEDIA_WIDTH, RowShape, SessionName, SystemBody,
    SystemLine, VIDEO_MAKING, VIDEO_PLAN, VIDEO_UNREACHABLE, VideoPlan,
};
pub use list::{
    ActiveCard, Agent, AppMember, AppRow, BADGE_MOST, Badges, ChannelGroup, DirectKind, DirectRow,
    ENDED_FOLD_AFTER, MachineRow, PersonRow, ROOM_QUIET_AFTER, SHOW_LABEL, Shelf, ShowChoice,
    SidebarSection, SidebarTab, THREAD_ACTIVE_FOR, Thread, Unseen, badge_label, room_label,
    room_parts,
};
pub use members::{MemberMachine, MemberRow, MemberSession};
pub use more::{CODE_LINES, More, PROSE_LINES, Shown, cut};
pub use names::{
    AVATAR_KEY, MENTIONS_OFFERED, Mention, MentionKind, NAME_KEY, PROFILES_PER_ASK,
    complete_mention, mention_query,
};
pub use new_message::{PEOPLE_OFFERED, Pickable, Started, TO, TYPE_A_NAME, pick_people};
pub use notify::{Level, Muted, NotifySettings, menu as conversation_menu};
pub use org::{OrgHeader, members_label, threads_label};
pub use people::{GUEST_PREFIX, Person, is_guest, machine_label, runs_on};
pub use plain::{
    APPS_SECTION, NO_OTHER_CHANNELS, PEOPLE_SECTION, QUOTE_MARK, ReactionChip, TO_MARK, UNASSIGNED,
};
pub use preview::Preview;
pub use profile::{
    BACKGROUND_AWAY, BARE_AWAY, Card, CardWork, IDLE_AFTER_SECS, IDLE_AWAY, LOCKED_AWAY,
    OFFLINE_AWAY, Presence, Profile, Standing, ago, ago_short, away_label, away_reason, away_title,
    card_time, clock, list_when, span,
};
pub use rich::{
    Block, CellAlign, LINK_LABEL_CHARS, ListItem, Span, SpanKind, TableSource, addressed,
    each_span, link_label, map_spans, media_link, rich, tables_in,
};
pub use row_menu::{LevelChoice, MuteChoice, RowMenu};
pub use run::{Here, Ran};
pub use scroll::{SCROLL_RULES, ScrollRules, Splice, splices};
pub use sealing::{
    ADD_KEY, CANT_SEAL_HERE, COPY_SEALED, KEYS_HERE, LOCK, NO_KEY_TO_SEAL, NO_KEYS_HERE, PASTE_KEY,
    REVEAL, SEAL, SEALED, SEALED_COPY_LASTS_SECS, SEALED_EXPLAINED, SEALED_FAILED, SEALED_MESSAGES,
    SEALED_NO_KEY, SEALS_IN_ONE, SEALS_ONLY_SAYING, SEALS_WITH, SealedPart, SealedSpan,
    SealedState, SealedTag, copy_sealed_label, key_added, no_random, seals,
};
pub use search::{
    Hit, HitPart, JUMP_ROWS, SEARCH_OLDER, SEARCHING_OLDER, Search, SearchHit, SearchView,
    ServerQuery, highlights, switch_rank,
};
pub use seek::{HOLE_LABEL, LATEST_LABEL};
pub use settings_list::{
    ADD_ORGANISATION, ADD_ORGANISATION_NAME, AWAY, AWAY_ELSEWHERE_NOTE, AWAY_NOTE, AWAY_WHY,
    DELETE_ACCOUNT, ORGANISATIONS, SERVER, SETTINGS, SETTINGS_ACCOUNT, SETTINGS_APP,
    SIGN_OUT_EVERYTHING, SYSTEM_NOTIFICATIONS, SettingsApp, SettingsGroup, SettingsKind,
    SettingsSection, THEMES, ThemeChoice, confirms_deletion, deletion_words, mute_org,
    mute_org_note, settings_group_title, settings_sections, sign_out_of, theme_dark, themes,
    type_to_confirm,
};
pub use sidebar::{ALL_MESSAGES, ListKind, ListOptions, ListRow, NO_MESSAGES};
pub use speaker::{
    AGENT_TAG, APP_TAG, Avatar, Speaker, YOU, agent_avatar, agent_label, agent_name, agent_session,
    app_name, possessive, sender_text, tool_mark,
};
pub use split::split_message;
pub use status::{Connection, Logged, Notice, Status};
pub use thread::{ThreadLink, thread_name};
pub use titles::{NOTHING_JOINED, NothingJoined, SERVER_TITLE, TitleOpens};
pub use typing::{EVERY as TYPING_EVERY, SESSION_TAG, TYPING_TAG, Typist, typing_words};
pub use view::{
    EVENT_MARK, Icon, Item, LANDING_KIND, Landing, Membership, Outcome, Parts, Pointer,
    RECONNECT_GAP, SenderKind, Stage, UPDATES_SHOWN, Update, ZIRC_LABEL, actor, app_label,
    day_label, is_system, people_only, sender_kind, trail_stubs,
};
pub use words::labels;
pub use words::{
    ACTIVITY, ACTIVITY_HINT, ADD_REACTION, ADVANCED, ASKING_ZIRC_DEV, ATTACH, BACK,
    BACK_TO_RESULTS, BROWSE_CHANNELS, CANCEL, CHANNEL_NAME, CHECK_THE_CODE, CHOOSE_A_SERVER, CLEAR,
    CLOSE_SEARCH, CONNECTION, CONTINUE_WITH_EMAIL, CONVERSATION_OPTIONS, COPY, COPY_CODE,
    COPY_LINK, COPY_MESSAGE_LINK, COPY_TEXT, COULDNT_SIGN_IN, DELETE, DELETE_MY_ACCOUNT, DISMISS,
    DONE, DOWNLOAD, EARLIER_MESSAGE, EARLIER_REPLY, EDIT_PROFILE, EVERYWHERE, GO_TO_NEWEST,
    GUEST_ACTIONS, HOME, JUMP_TO, JUMP_TO_PROMPT, MACHINES, MORE, MORE_PEOPLE_ONLY, NEW_CHANNEL,
    NEW_DIVIDER, NEW_MESSAGE, NEW_TAG, NO_CHANNELS, NO_DIRECTS, NO_DIRECTS_HINT, NOT_NOW,
    NOTHING_YET, NOTIFICATION_CHANNEL, PEOPLE_ONLY, PICK_A_CONVERSATION, PROFILE, REACT_LABEL,
    REPLY_LABEL, REPLYING_TO, RESULTS, RETRY, SAID_BY, SAVE, SEARCH, SEARCH_IN, SEND, SENDING,
    SETTINGS_ALL, SHOWN_PATHS, SIGNED_IN, STOP_REPLYING, THREADS, TRY_AGAIN, TURN_ON_NOTIFICATIONS,
    VERSION, Words, YOU_PAGE, YOUR_NAME, activity_empty, couldnt_label, details_label,
    guests_label, in_label, mentions_label, menu_bar, more_paths_label, new_label, nothing_called,
    profile_label, react_with_label, removed_label, scope_label, sharing_label,
    show_more_paths_label, signin_failed, version_label, words,
};
pub use working::{Intending, WorkingPerson, WorkingSet};
pub use zirc_proto::search::{
    Chip as SearchChip, Query as SearchQuery, Scope as SearchScope, SearchSort,
    Unreadable as UnreadableSearch,
};

/// Who's in which channel, and away (`crate::presence`), apart from a person's dot,
/// [`Presence`].
use crate::presence::Presence as PresenceMap;

/// The longest message text sent on one line; longer goes as a multiline message or in
/// pieces.
const MAX_TEXT_BYTES: usize = 400;
/// The longest list of channels in one `JOIN`.
const MAX_JOIN_BYTES: usize = 400;
/// What our WHO queries carry, to tell their answers from others'.
const WHO_TOKEN: &str = "42";
/// Ergo's services. What they say to us goes in the server's conversation, and what we say
/// to them is never shown: a message to `NickServ` can carry a password.
const SERVICES: &[&str] = &["NickServ", "ChanServ", "HostServ", "HistServ"];
/// The `IRCv3` client tags reactions travel in: this one gives one, as what
/// [`Chat::react`] sends says when it isn't taking one back.
pub const REACT: &str = "+draft/react";
const UNREACT: &str = "+draft/unreact";
const REPLY: &str = "+draft/reply";
/// The client-only tag on a message that corrects an earlier one of ours, by the server's
/// id ([`Chat::say_edit`]): zirc's apps show the new words in its place, marked
/// [`EDITED`]; plain IRC clients, as an ordinary line.
const EDIT: &str = "+draft/edit";
/// How long after saying it a message can be corrected ([`Chat::say_edit`]); deleting it has
/// no such limit.
pub const EDIT_WINDOW: chrono::TimeDelta = chrono::TimeDelta::minutes(20);
/// The organisation-wide channel.
pub use zirc_proto::ORG_CHANNEL;

/// The channels a person is put in once, on first connecting to an organisation: when they
/// are in no channel at all and asked for none. After that they manage their own list, and
/// the server keeps an always-on account's channels. A team's configuration will be able to
/// extend this; the repositories' channels are found with [`Chat::browse`], not pushed. Known
/// edge: someone who has left every channel is put in these again at the next launch.
pub const DEFAULT_CHANNELS: &[&str] = &[ORG_CHANNEL];

/// The `IRCv3` capability for metadata, which apps ask for.
pub const METADATA_CAP: &str = "draft/metadata-2";

/// The capability that tells a person's new real name, which is the name their apps send
/// when their profile's changes (`SETNAME`).
pub const SETNAME_CAP: &str = "setname";

/// What an app that keeps a [`Chat`] asks for on top of zirc-line's capabilities:
/// `echo-message`, so its own messages come back with the server's id and time (without it,
/// reactions to them have nothing to name); `extended-join`, so a join says the account and
/// real name, where a zirc machine names its owner and a guest who comes in later is known as
/// one; metadata, for how people show and whether a guest is signed in to zirc.dev;
/// `setname`, for real names that change; and `chghost`, for a machine's host, which says whose
/// it is, set while it's on. Every app asks for these, the web client included.
pub const APP_CAPS: &[&str] = &[
    "echo-message",
    "extended-join",
    METADATA_CAP,
    SETNAME_CAP,
    "chghost",
];

/// What's been said since the newest message we had of a conversation, being asked for.
#[derive(Debug)]
struct Gap {
    /// The newest message's id we had; `None` for a conversation opened with none, which
    /// pages back until a screenful of what people said is in.
    since: Option<String>,
    /// How many pages have been asked for.
    pages: usize,
    /// What it was asked for.
    ask: Ask,
}

/// What a page of history ([`Gap`]) was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ask {
    /// What's newest, or what was said since the newest we had: page after page until it
    /// meets what's held.
    Catch,
    /// By the person reaching the top of what's loaded ([`Chat::older`]): one page.
    Older,
    /// The messages around the one a link or a search names, the server's id here
    /// ([`Chat::open_at`]): one page, which may leave a [`Conversation::hole`] below it.
    Around(String),
    /// The page after a [`Conversation::hole`], as the person reaches it ([`Chat::newer`]).
    Newer,
}

/// A page of history filling a [`Gap`], as it arrives.
#[derive(Debug)]
struct Page {
    target: String,
    since: Option<String>,
    /// How many messages it had.
    count: usize,
    /// The id of its oldest.
    oldest: Option<String>,
    /// Whether it went back as far as the newest we had.
    reached: bool,
    /// The id of its newest.
    newest: Option<String>,
    /// Whether it had a message the conversation held already, so it meets what's held.
    known: bool,
}

/// What the connection allows, as registration settled it.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is a capability the server enabled or didn't, independent of the others"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The nick the server gave us.
    pub nick: String,
    /// Whether the server keeps history we can ask for.
    pub chathistory: bool,
    /// Whether the server echoes our own messages back, with their ids and times.
    pub echo: bool,
    /// Whether the server keeps how far we've read each conversation, for all our
    /// connections at once (`draft/read-marker`).
    pub read_marker: bool,
    /// Whether the server lets messages be trimmed from its history, and says when they
    /// are ([`crate::trim`]).
    pub trim: bool,
    /// What the server takes in one multiline message, when it takes them.
    pub multiline: Option<Limits>,
    /// Whether the server tells metadata it's asked to follow (`draft/metadata-2`), such as
    /// whether a guest is signed in to zirc.dev.
    pub metadata: bool,
}

impl Link {
    /// The link of a connection registered as `nick`, with the capabilities `has_cap` says
    /// the server enabled and its multiline limits, as registration settled them
    /// ([`zirc_line::registration::Registered`]).
    #[must_use]
    pub fn new(nick: &str, has_cap: impl Fn(&str) -> bool, multiline: Option<Limits>) -> Self {
        Self {
            nick: nick.to_owned(),
            chathistory: has_cap("draft/chathistory") && has_cap("batch"),
            echo: has_cap("echo-message"),
            read_marker: has_cap("draft/read-marker"),
            trim: has_cap(crate::trim::CAP),
            multiline,
            metadata: has_cap(METADATA_CAP),
        }
    }
}

/// What differs between zirc's apps.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is a way one app differs from another, independent of the others"
)]
#[derive(Debug, Clone)]
pub struct Options {
    /// How many messages of history to ask for when joining a conversation.
    pub history: usize,
    /// How many of what people said a conversation opened without history pages back for,
    /// past machines' events, which fold away: a screenful.
    pub screenful: usize,
    /// How many entries a conversation keeps; older ones are dropped, a batch at a time once
    /// it holds an eighth more.
    pub max_entries: usize,
    /// What names the multiline batches this client sends, such as `tui`.
    pub batch_prefix: String,
    /// Whether joining the first channel shows it in place of the server's conversation.
    /// A channel the person asked to join is shown either way.
    pub show_first_channel: bool,
    /// The app's name, such as `desktop`, sent with what the person says in
    /// [`CLIENT_TAG`] so others can see where it was sent from. `None` sends nothing.
    pub client: Option<String>,
    /// Whether to tell others when the person is typing ([`Chat::typed`]): unless they
    /// turned it off.
    pub send_typing: bool,
    /// Whether to ask the server for its channels and join them, as every app does in an
    /// organisation. Without, only the channels [`Chat::new`] was given are joined: the web
    /// client's support room, where a visitor sees that one channel and nothing else.
    pub list_channels: bool,
    /// Whether opening a conversation at a message older than what's loaded
    /// ([`Chat::open_at`]) asks for the messages around it in one go (`CHATHISTORY
    /// AROUND`), leaving a hole between them and what's newest ([`Chat::hole`]) that the
    /// app fills as the person reaches it ([`Chat::newer`]). Without, it pages back until
    /// the message is there: for an app that doesn't draw the hole yet.
    pub around: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            history: 200,
            screenful: 40,
            max_entries: 2000,
            batch_prefix: "zirc".to_owned(),
            show_first_channel: true,
            client: None,
            send_typing: true,
            list_channels: true,
            around: false,
        }
    }
}

/// Where the person reads the conversation shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reading {
    /// At its end, seeing what comes.
    AtEnd,
    /// Further up, where what comes is new.
    Up,
}

/// Everything a chat client knows about its server.
#[expect(
    clippy::struct_excessive_bools,
    reason = "a state machine's flags, each about a different thing and set by a different event"
)]
#[derive(Debug)]
pub struct Chat {
    /// What time it is, replaceable in tests: [`crate::clock::now`] unless replaced.
    pub clock: fn() -> DateTime<Utc>,
    /// Fills its bytes with random ones for sealing, saying whether it could: the system's,
    /// which each app gives, as this crate runs in the browser too. [`no_random`] unless
    /// given, and nothing is sealed.
    pub random: fn(&mut [u8]) -> bool,
    options: Options,
    me: String,
    server: String,
    link: Option<Link>,
    /// The conversations: the server's first, then channels, then private ones, each
    /// sorted by name.
    conversations: Vec<Conversation>,
    /// Where each of `conversations` is, by folded name: [`Chat::position`] in one lookup,
    /// as the sidebar asks it hundreds of times a frame. Rebuilt wherever one is added or removed.
    index: HashMap<String, usize>,
    /// Where the rooms under each channel are in `conversations`, by the channel's folded
    /// name: `#web/agents` under `#web`, so one channel's rooms are found without looking
    /// through every conversation. Rebuilt with `index`.
    rooms: HashMap<String, Vec<usize>>,
    selected: usize,
    /// Whether the server's own conversation is shown as the Server channel
    /// ([`Chat::show_server`]), not as the stand-in for nothing shown that it is otherwise.
    server_shown: bool,
    /// Until when every reply the server sends is the Server channel's too: a few seconds after
    /// a line was typed there ([`Chat::server_window`]).
    raw_until: Option<DateTime<Utc>>,
    /// Channels to join besides the server's own.
    joins: Vec<String>,
    /// What was said while not connected, by where to, oldest first: it goes once that
    /// conversation is joined again, a direct message's once connected, so what's typed
    /// offline waits rather than being refused, in every app (the user's choice, 20 Nov 2026).
    held: Vec<(String, Vec<Line>)>,
    /// The server's channels and how many are in each, from the last complete `LIST`, and
    /// the one coming in.
    listed: Vec<(String, usize)>,
    listing: Vec<(String, usize)>,
    /// Whether this connection's first `LIST` has been answered, which is when a person in
    /// no channel is put in the [`DEFAULT_CHANNELS`], never later.
    listed_once: bool,
    /// Whether the server keeps our channels, as it does an always-on account's: it has put
    /// us in one unasked on connecting, on this connection or an earlier one. What it then
    /// leaves us out of was left on another device while this one was away, and stays left.
    keeps_channels: bool,
    /// The channels, folded, this connection has asked to join before its first `LIST` was
    /// answered: joining those is our doing, not the server keeping them.
    asked: HashSet<String>,
    /// Whether `joins` have been asked for once. A server that keeps our channels keeps
    /// those too, so they aren't asked for again: one left since stays left.
    joins_sent: bool,
    /// Open batches of history, and of conversations with history.
    history: HashSet<String>,
    targets: HashSet<String>,
    /// Folded conversation → the gap being filled since the newest message we had of it,
    /// on connecting again.
    gaps: HashMap<String, Gap>,
    /// Open batches filling a gap: batch reference → what's arrived in it.
    filling: HashMap<String, Page>,
    /// The message a conversation was opened at, while it's looked for ([`Chat::open_at`]).
    seeking: Option<seek::Seek>,
    /// That message, once it's loaded, for the app to bring into view ([`Chat::found`]).
    found: Option<String>,
    /// A channel to show once joined, because the person asked to join it.
    show_on_join: Option<String>,
    /// Whether the channel shown was shown as the first joined, and nothing's been shown
    /// since: one earlier in the order takes its place, whichever the server joins first.
    first_shown: bool,
    /// Where the "New" divider goes in the conversation shown: the first message it hadn't
    /// read when it was shown ([`Chat::divider`]).
    divider: Option<String>,
    /// The divider whose pill has said its piece: the New line came on screen, or the pill
    /// was closed ([`Chat::saw_divider`]). A newer divider's pill shows again.
    divider_seen: Option<String>,
    /// Where the person reads the conversation shown: the apps say, [`Chat::scrolled`].
    reading: Reading,
    /// Whether anyone is looking at the conversation shown: not while the app shows another
    /// organisation ([`Chat::set_looking`]).
    looking: bool,
    /// Whether the app is in front: its window active, or the phone's app in the foreground
    /// ([`Chat::set_in_front`]).
    in_front: bool,
    /// Whether the computer's screen is locked, as its app last said ([`Chat::set_idle`]).
    locked: bool,
    /// The long messages the person opened or cut again ([`Chat::toggle_more`]).
    opened: more::Opened,
    /// The keys for sealed parts, and the parts revealed ([`Chat::set_keys`]).
    sealing: sealing::Sealing,
    /// Puts received multiline messages back together, before anything else sees them.
    multiline: Batches,
    sent_batches: u64,
    presence: PresenceMap,
    /// What's known of files changed by two at once ([`Chat::both_changing`]).
    both: both::Known,
    /// Folded nick → IRC real name, from `extended-join` and WHO.
    realnames: HashMap<String, String>,
    /// Folded nick → whose machine it is, its owner's handle, as its host says
    /// (`machine/<owner>`, which only the server sets): from every line it sends, WHO and
    /// `CHGHOST`. A person, or a machine whose host doesn't say, isn't here.
    machines: HashMap<String, String>,
    /// Folded nick → the account they're logged in to, `None` for none, as extended-join,
    /// WHOX and messages' account tags say: what tells a guest from a member.
    accounts: HashMap<String, Option<String>>,
    /// Folded guest nick → whether they're signed in to a zirc.dev account, as the
    /// organisation's server tells the app.
    guest_accounts: HashMap<String, bool>,
    /// How people show, as their profiles say ([`Chat::shown_name`]).
    names: names::Names,
    /// Folded nicks already asked of zirc.dev's profiles ([`Chat::profiles_to_ask`]).
    asked_profiles: std::collections::HashSet<String>,
    /// Agent session id → the name it goes by, as its messages say: see
    /// [`Chat::session_label`].
    /// The nick each agent session was first heard from, which alone names and titles it.
    session_from: HashMap<String, String>,
    session_names: HashMap<String, String>,
    /// Agent session id → the tool it runs in, as its messages say (`claude-code`).
    session_tools: HashMap<String, String>,
    /// Agent session id → the gallery picture it chose, as its messages say (`rocket`).
    session_pictures: HashMap<String, String>,
    /// Agent session id → its title card, as its machine last said it, and when
    /// ([`Chat::agent_card`]).
    session_titles: HashMap<String, (DateTime<Utc>, zirc_proto::session::SessionCard)>,
    /// Folded machine nick → which of its agent sessions it last said are live
    /// ([`Chat::session_live`]).
    live: HashMap<String, live::MachineLive>,
    /// Whether we're one of the organisation's owners or admins, as zirc.dev says.
    admin: bool,
    /// Whether we're one of its owners, as zirc.dev says.
    owner: bool,
    /// Folded nick → away message (maybe empty), for those away: from `away-notify`, WHO
    /// and the server's replies. Ourselves included.
    away: HashMap<String, String>,
    /// Folded nick → when they were last active, for those away or offline whose going away
    /// we saw: from the time of their `AWAY` line and what it said ([`Chat::active_at`]).
    last_active: HashMap<String, DateTime<Utc>>,
    /// Whether we last asked to be away or back, until the server answers: to say what
    /// came of it.
    asked_away: Option<bool>,
    /// Whether [`Status::away_here`] is this app's own doing, from [`Chat::set_idle`], not asked
    /// for by the person: only then does coming back clear it.
    auto_away: bool,
    /// Folded channel and nick → when their latest working set was said, so history
    /// taken in after it never replaces it.
    working_at: HashMap<(String, String), DateTime<Utc>>,
    /// Folded nick → what a WHOIS said of them.
    whois: HashMap<String, profile::Whois>,
    /// Groups we made and haven't joined yet, by folded name: who to let in once we have.
    making: HashMap<String, Vec<String>>,
    /// Folded group name → who we let in, joined yet or not: who a group is with.
    invited: HashMap<String, Vec<String>>,
    /// How many groups we've made, to name each apart.
    made: u64,
    /// Folded conversation name → when something was last said in it, as the server's
    /// `CHATHISTORY TARGETS` said ([`Chat::last_said`]).
    active: HashMap<String, DateTime<Utc>>,
    /// The person's starred conversations, in order ([`Chat::star`]).
    stars: Vec<String>,
    /// The thresholds the person picked that aren't a conversation's default, by folded
    /// name ([`Chat::set_threshold`]).
    thresholds: BTreeMap<String, zirc_proto::Level>,
    /// The organisation's name, as the app signed in to it says ([`Chat::set_org_name`]).
    org_name: Option<String>,
    /// What the server calls its network, `NETWORK=` in its `005`: the organisation's slug.
    network: Option<String>,
    /// The parts of the sidebar the person folded away ([`Chat::set_collapsed`]).
    collapsed: std::collections::HashSet<list::Shelf>,
    /// The section the sidebar shows ([`Chat::tab`]).
    tab: list::SidebarSection,
    /// Folded direct message name → when the person closed it ([`Chat::close_direct`]).
    closed: HashMap<String, DateTime<Utc>>,
    /// The organisation's kept guests, as zirc.dev lists them to owners and admins
    /// ([`Chat::set_kept_guests`]).
    kept_guests: Vec<guest_rooms::KeptGuest>,
    /// Folded channel → whether the person opened its folded guests' rooms
    /// ([`Chat::set_guest_rooms_open`]).
    guest_rooms_open: HashMap<String, bool>,
    /// How much each conversation notifies ([`Chat::level`]).
    notify: notify::NotifySettings,
    /// Folded nicks of the people the person blocked ([`Chat::set_blocked`]).
    blocked: std::collections::BTreeSet<String>,
    /// Folded conversation name → who is typing there ([`Chat::typing`]).
    typing: HashMap<String, Vec<Typist>>,
    /// Folded conversation name → when we last said the person is typing there.
    sent_typing: HashMap<String, DateTime<Utc>>,
    /// Messages that arrived live since [`Chat::take_fresh`], with their conversation's
    /// name: what might notify.
    fresh: Vec<(String, Entry)>,
    /// The connection, and what the server and its services said.
    status: Status,
    /// Where pictures and files load from ([`Chat::set_picture_site`]).
    picture_site: face::PictureSite,
    /// What colours code blocks' lines, if the app opted in ([`Chat::set_highlighter`]).
    highlighter: Option<flat::Highlighter>,
    /// Each repository's forge and checkout, and the editor, that code links to
    /// ([`Chat::set_checkouts`]).
    code: code_ref::Known,
}

impl Chat {
    /// A session on `server`, as `nick` until the server says otherwise, that will join
    /// `joins` besides the channels the server lists.
    #[must_use]
    pub fn new(nick: &str, server: &str, joins: Vec<String>, options: Options) -> Self {
        Self {
            clock: crate::clock::now,
            random: no_random,
            options,
            me: nick.to_owned(),
            server: server.to_owned(),
            link: None,
            conversations: vec![Conversation::new(server, Kind::Server)],
            index: HashMap::from([(fold(server), 0)]),
            rooms: HashMap::new(),
            selected: 0,
            server_shown: false,
            raw_until: None,
            joins,
            held: Vec::new(),
            listed: Vec::new(),
            listing: Vec::new(),
            listed_once: false,
            keeps_channels: false,
            asked: HashSet::new(),
            joins_sent: false,
            history: HashSet::new(),
            gaps: HashMap::new(),
            filling: HashMap::new(),
            seeking: None,
            found: None,
            targets: HashSet::new(),
            show_on_join: None,
            first_shown: false,
            divider: None,
            divider_seen: None,
            reading: Reading::AtEnd,
            looking: true,
            in_front: true,
            locked: false,
            opened: more::Opened::default(),
            sealing: sealing::Sealing::default(),
            multiline: Batches::default(),
            sent_batches: 0,
            presence: PresenceMap::default(),
            both: both::Known::default(),
            realnames: HashMap::new(),
            machines: HashMap::new(),
            accounts: HashMap::new(),
            guest_accounts: HashMap::new(),
            names: names::Names::new(),
            asked_profiles: std::collections::HashSet::new(),
            session_from: HashMap::new(),
            session_names: HashMap::new(),
            session_tools: HashMap::new(),
            session_pictures: HashMap::new(),
            session_titles: HashMap::new(),
            live: HashMap::new(),
            admin: false,
            owner: false,
            away: HashMap::new(),
            last_active: HashMap::new(),
            asked_away: None,
            auto_away: false,
            working_at: HashMap::new(),
            whois: HashMap::new(),
            making: HashMap::new(),
            invited: HashMap::new(),
            made: 0,
            active: HashMap::new(),
            stars: Vec::new(),
            thresholds: BTreeMap::new(),
            org_name: None,
            network: None,
            collapsed: std::collections::HashSet::new(),
            tab: list::SidebarSection::Channels,
            closed: HashMap::new(),
            kept_guests: Vec::new(),
            guest_rooms_open: HashMap::new(),
            notify: notify::NotifySettings::default(),
            blocked: std::collections::BTreeSet::new(),
            typing: HashMap::new(),
            sent_typing: HashMap::new(),
            fresh: Vec::new(),
            status: Status::default(),
            picture_site: face::PictureSite::default(),
            highlighter: None,
            code: code_ref::Known::default(),
        }
    }

    /// Our nick.
    #[must_use]
    pub fn me(&self) -> &str {
        &self.me
    }

    /// The server, as its conversation is named.
    #[must_use]
    pub fn server(&self) -> &str {
        &self.server
    }

    /// The conversations: the server's first, then channels, then private ones.
    #[must_use]
    pub fn conversations(&self) -> &[Conversation] {
        &self.conversations
    }

    /// The conversation called `name`.
    #[must_use]
    pub fn conversation(&self, name: &str) -> Option<&Conversation> {
        self.position(name).map(|at| &self.conversations[at])
    }

    /// Where the conversation called `name` is in [`Chat::conversations`].
    #[must_use]
    pub fn position(&self, name: &str) -> Option<usize> {
        self.index.get(&fold(name)).copied()
    }

    /// Records where each conversation is, after one is added or removed.
    pub(crate) fn reindex(&mut self) {
        self.index = self
            .conversations
            .iter()
            .enumerate()
            .map(|(at, c)| (fold(&c.name), at))
            .collect();
        self.rooms.clear();
        for (at, conversation) in self.conversations.iter().enumerate() {
            if let Some((channel, _)) = room_parts(&conversation.name) {
                self.rooms.entry(fold(channel)).or_default().push(at);
            }
        }
    }

    fn push(&mut self, at: usize, entry: Entry) {
        let max = self.options.max_entries;
        self.conversations[at].insert(entry, max);
    }

    /// The conversation called `name`, made if there isn't one, in its place in the order.
    fn ensure(&mut self, name: &str, kind: Kind) -> usize {
        if let Some(at) = self.position(name) {
            return at;
        }
        let key = (kind, fold(name));
        let at = self
            .conversations
            .partition_point(|c| (c.kind, fold(&c.name)) <= key);
        let mut conversation = Conversation::new(name, kind);
        if let Some(level) = self.thresholds.get(&fold(&conversation.name)) {
            conversation.threshold = *level;
        }
        self.conversations.insert(at, conversation);
        self.reindex();
        if at <= self.selected && self.conversations.len() > 1 {
            self.selected += 1;
        }
        at
    }

    fn time_of(&self, line: &Line) -> DateTime<Utc> {
        line.tag("time")
            .and_then(|time| DateTime::parse_from_rfc3339(time).ok())
            .map_or_else(self.clock, |time| time.with_timezone(&Utc))
    }
}

/// The nick in a line's source, or the server's name.
/// Whose machine sent `line`, its owner's handle, as its host says (`machine/<owner>`).
fn owner_by_host(line: &Line) -> Option<&str> {
    let (_, host) = line.source.as_deref()?.split_once('@')?;
    zirc_proto::names::owner_from_host(host)
}

fn nick_of(line: &Line) -> &str {
    let source = line.source.as_deref().unwrap_or_default();
    source.split_once('!').map_or(source, |(nick, _)| nick)
}

fn is_service(nick: &str) -> bool {
    SERVICES.iter().any(|s| s.eq_ignore_ascii_case(nick))
}

fn kind_of(target: &str) -> Kind {
    if is_channel(target) {
        Kind::Channel
    } else {
        Kind::Query
    }
}

fn some(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
}

/// A time as `CHATHISTORY` takes it.
fn stamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Whether `channel` is one people join by choice, never by being listed: a room under
/// another, as `#support/login`.
fn by_choice(channel: &str) -> bool {
    channel.contains('/')
}

/// `AWAY` with `message`, or without one to say we're back.
fn away_line(message: Option<&str>) -> Vec<Line> {
    match message {
        Some(message) => vec![Line::new("AWAY", [message])],
        None => vec![Line::new("AWAY", [""; 0])],
    }
}

/// `JOIN`s for `channels`, as few as fit.
fn joins(channels: &[String]) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut list = String::new();
    for channel in channels {
        if !list.is_empty() && list.len() + 1 + channel.len() > MAX_JOIN_BYTES {
            lines.push(Line::new("JOIN", [std::mem::take(&mut list)]));
        }
        if !list.is_empty() {
            list.push(',');
        }
        list.push_str(channel);
    }
    if !list.is_empty() {
        lines.push(Line::new("JOIN", [list]));
    }
    lines
}

/// `text` as several `PRIVMSG`s, a line of it at least each, for a server that doesn't take
/// multiline messages. Blank lines go.
/// What a PRIVMSG's or NOTICE's `body` is as an entry, and the words it says, for
/// mentions; `None` for what isn't shown, such as a CTCP request.
fn entry_of(body: &str, notice: bool) -> Option<(EntryKind, String)> {
    if notice {
        let text = strip_formatting(body);
        return Some((EntryKind::Notice(text.clone()), text));
    }
    Some(match read(body) {
        Body::Ctcp => return None,
        Body::Text(text) => (EntryKind::Message(text.clone()), text),
        Body::Action(text) => (EntryKind::Action(text.clone()), text),
        Body::Unreadable(text) => (EntryKind::Unreadable(text), String::new()),
        Body::Zirc(mut message) => {
            // A decision is said in the current house style (`Decision::text`), whatever
            // version of zirc posted it: its data has everything the text says.
            if let Some(decision) = zirc_proto::decision::Decision::of(&message.event) {
                message.event = decision.event();
            }
            let words = match &message.event {
                Event::Text { body } => body.clone(),
                _ => String::new(),
            };
            (EntryKind::Zirc(*message), words)
        }
    })
}

/// What an entry of `kind` keeps of the PRIVMSG or NOTICE `body` with its formatting: a
/// message's, action's or notice's text, when it has any. See [`formatted`].
fn formatted_text(kind: &EntryKind, body: &str) -> Option<String> {
    if !matches!(
        kind,
        EntryKind::Message(_) | EntryKind::Action(_) | EntryKind::Notice(_)
    ) {
        return None;
    }
    let text = body
        .strip_prefix("\x01ACTION ")
        .map_or(body, |action| action.trim_end_matches('\x01'));
    formatted(text)
}

/// One message as [`Chat::said`] sends it.
enum Outgoing {
    /// Whole, as one multiline message: its text.
    Batch(String),
    /// On a line of its own: its PRIVMSG body.
    Line(String),
}

/// `text` as the bodies of PRIVMSGs of a line each, for a server without multiline
/// messages: each line on its own, cut where it's too long for one.
fn pieces(text: &str, body: &dyn Fn(&str) -> String) -> Vec<String> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        // Room for an action's framing.
        .flat_map(|line| split_for_line(line, MAX_TEXT_BYTES - 10))
        .map(|piece| body(&piece))
        .collect()
}

#[cfg(test)]
mod tests;
