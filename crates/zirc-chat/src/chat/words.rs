//! Small phrases every app says the same way, said once here: what the composer says
//! while replying, where a search looks and where a match is, how many paths an event
//! leaves out or folds away, how many notifications are new, what Jump to says when nothing matches, and
//! the sidebar's names that aren't a [`Shelf`](super::Shelf)'s. [`Words`] hands the ones
//! that take no argument to an app whole, so it types none of them.

use serde::Serialize;
use ts_rs::TS;

use super::{agent_card, finder, notify::menu, plain, search, settings_list, speaker};

pub mod labels;

/// What the composer says while it replies, before who it answers: `Replying to` bob.
pub const REPLYING_TO: &str = "Replying to";

/// What opens the list of channels to join, in the sidebar and as Jump to's heading for it.
pub const BROWSE_CHANNELS: &str = "Browse channels";

/// What makes a channel, on the sidebar's row and the header's button for it, and the title
/// of where its name is asked (the user's choice, 13 Nov 2026). Always there, with no
/// channel joined or listed too, so nobody is left in an empty server.
pub const NEW_CHANNEL: &str = "New channel";

/// What the field for a new channel's name says while it's empty.
pub const CHANNEL_NAME: &str = "Channel name";

/// Where a search looks when it isn't in a conversation.
pub const EVERYWHERE: &str = "Everywhere";

/// Where what was said to the person gathers: a tab, its page's title, the bell's name.
pub const ACTIVITY: &str = "Activity";

/// Under [`NOTHING_YET`] on [`ACTIVITY`]'s empty page: what will show there.
pub const ACTIVITY_HINT: &str =
    "When someone mentions you, replies to you or reacts to what you said, it shows here.";

/// [`ACTIVITY`]'s empty line where there's one line for it, as in a popover: `Nothing yet.`
/// and [`ACTIVITY_HINT`].
#[must_use]
pub fn activity_empty() -> String {
    format!("{NOTHING_YET}. {ACTIVITY_HINT}")
}

/// What folds away a sign-in's server and connection, which only fixing one needs.
pub const ADVANCED: &str = "Advanced";

/// While zirc.dev is asked: for a sign-in to start, for a profile, for what deleting an
/// account does.
pub const ASKING_ZIRC_DEV: &str = "Asking zirc.dev…";

/// What opens the emoji to react to a message with, where it's a button's name: the `+`
/// after a message's reactions, its hover bar's button.
pub const ADD_REACTION: &str = "Add reaction";

/// The composer's button that adds a file.
pub const ATTACH: &str = "Attach";

/// What goes back a step: out of a question a card's button asks, out of a list.
pub const BACK: &str = "Back";

/// What a screen reader says of [`RESULTS`]' pill.
pub const BACK_TO_RESULTS: &str = "Back to search results";

/// What turns a question down, and stops what waits.
pub const CANCEL: &str = "Cancel";

/// What empties a field, for a screen reader.
pub const CLEAR: &str = "Clear";

/// What shuts search, for a screen reader.
pub const CLOSE_SEARCH: &str = "Close search";

/// The label of how a sign-in's connection stands, under [`ADVANCED`].
pub const CONNECTION: &str = "Connection";

/// What a screen reader calls a conversation's row's long press, which opens its options:
/// the same menu its header's [`MORE`] opens (the user's choice, 17 Nov 2026).
pub const CONVERSATION_OPTIONS: &str = "Conversation options";

/// A code block's own button that copies it.
pub const COPY: &str = "Copy";

/// A message's menu: copies a code block of it.
pub const COPY_CODE: &str = "Copy code";

/// A message's menu: copies a link it holds.
pub const COPY_LINK: &str = "Copy link";

/// A message's menu: copies a link to the message itself.
pub const COPY_MESSAGE_LINK: &str = "Copy link to message";

/// A message's menu: copies what it says.
pub const COPY_TEXT: &str = "Copy text";

/// A message's menu: deletes it.
pub const DELETE: &str = "Delete";

/// The button that deletes the account, once what that does is read.
pub const DELETE_MY_ACCOUNT: &str = "Delete my account";

/// What puts away an error once it's read, such as a send's that failed.
pub const DISMISS: &str = "Dismiss";

/// What shuts a page once it's finished with, and what a card's button says it did when
/// the integration says nothing.
pub const DONE: &str = "Done";

/// A file's button that fetches it.
pub const DOWNLOAD: &str = "Download";

/// What opens the person's own name and picture, on their card and their page.
pub const EDIT_PROFILE: &str = "Edit profile";

/// What a screen reader says of the button back to a conversation's end while nothing new
/// waits.
pub const GO_TO_NEWEST: &str = "Go to the newest";

/// What a screen reader calls a guest's row of actions in their conversation's header.
pub const GUEST_ACTIONS: &str = "Guest actions";

/// The first tab on a phone, and its page's title.
pub const HOME: &str = "Home";

/// The quick switcher's name, for a screen reader.
pub const JUMP_TO: &str = "Jump to";

/// What the quick switcher's empty field says, and what opens it.
pub const JUMP_TO_PROMPT: &str = "Jump to…";

/// The button that opens a menu of what else can be done, on a message and in a header.
pub const MORE: &str = "More";

/// What a screen reader says of a header's [`MORE`] while [`PEOPLE_ONLY`] is on.
pub const MORE_PEOPLE_ONLY: &str = "More, showing people only";

/// What starts a direct message to someone not picked yet: a button, a menu item, its
/// sheet's title.
pub const NEW_MESSAGE: &str = "New message";

/// What marks the first message unread when a conversation was opened, on a tag at the end
/// of its line, as every app draws it (the user's choice, 21 Nov 2026: a tag, as `AGENT`).
pub const NEW_TAG: &str = "NEW";

/// What puts a question off, such as the offer to turn notifications on.
pub const NOT_NOW: &str = "Not now";

/// [`ACTIVITY`]'s empty line.
pub const NOTHING_YET: &str = "Nothing yet";

/// A person's machines, as a heading on their card.
pub const MACHINES: &str = "Machines";

/// The channel list's empty line, once connected.
pub const NO_CHANNELS: &str = "No channels yet.";

/// Direct messages' empty line.
pub const NO_DIRECTS: &str = "No direct messages yet";

/// A header's menu: shows only what people said.
pub const PEOPLE_ONLY: &str = "People only";

/// The title of the person's name and picture, while it loads too.
pub const PROFILE: &str = "Profile";

/// What opens the emoji to react to a message with.
pub const REACT_LABEL: &str = "React";

/// What answers a message: in its menu, on its hover bar, on a notification.
pub const REPLY_LABEL: &str = "Reply";

/// The list of a search's matches, and the pill in a conversation that goes back to it.
pub const RESULTS: &str = "Results";

/// What tries to connect again from the offline line.
pub const RETRY: &str = "Retry";

/// The title of the list of people a search can be narrowed to.
pub const SAID_BY: &str = "Said by";

/// Search's name: its page's title, what opens it, its field while the core has nothing
/// longer to say.
pub const SEARCH: &str = "Search";

/// The title of the list of conversations a search can be narrowed to, and of the control
/// that says where it looks.
pub const SEARCH_IN: &str = "Search in";

/// The composer's button that sends, and a notification's that sends a reply.
pub const SEND: &str = "Send";

/// Under a card's buttons, while the integration is told of a press.
pub const SENDING: &str = "Sending…";

/// What opens the settings no one organisation owns, under the list of them.
pub const SETTINGS_ALL: &str = "Settings for all organisations";

/// The label of who a sign-in is, in an organisation's settings.
pub const SIGNED_IN: &str = "Signed in";

/// What shuts the composer's [`REPLYING_TO`] line, for a screen reader.
pub const STOP_REPLYING: &str = "Stop replying";

/// A channel's threads: the list's heading and the button that shows it.
pub const THREADS: &str = "Threads";

/// What tries again what failed: a send, on the error's line, or a sign-in. The web page
/// types it too for its sign-in, drawn before its wasm is loaded (`oss/web/src/App.tsx`).
pub const TRY_AGAIN: &str = "Try again";

/// What asks the system to let the app notify: the phones' question's title, the
/// desktop's button for it in Settings.
pub const TURN_ON_NOTIFICATIONS: &str = "Turn on notifications";

/// The label of the app's version, in About.
pub const VERSION: &str = "Version";

/// What a name's field says before one is given, and what a screen reader calls the note
/// of the name a guest got. The web page types it too on its guest form, drawn before its
/// wasm is loaded (`oss/web/src/guest.tsx`).
pub const YOUR_NAME: &str = "Your name";

/// The person's own page and its tab, and the row that stands for them. A preview's
/// speaker is [`Preview`](super::Preview)'s own.
pub const YOU_PAGE: &str = "You";

/// Under [`ASKING_ZIRC_DEV`], once zirc.dev has a sign-in waiting: what to check, before
/// the code it shows, and [`CANCEL`] under it (the user's choice, 17 Nov 2026: the Mac's).
pub const CHECK_THE_CODE: &str = "Check that the page in your browser shows";

/// What a sign-in that failed says, before why ([`signin_failed`]).
pub const COULDNT_SIGN_IN: &str = "Couldn't sign in";

/// What a reply says of the message it answers once that's no longer kept, in place of who
/// said it and what (the user's choice, 17 Nov 2026: every app, web too).
pub const EARLIER_REPLY: &str = "a reply to an earlier message";

/// [`EARLIER_REPLY`]'s message, where the composer's reply line names what it answers.
pub const EARLIER_MESSAGE: &str = "an earlier message";

/// Under [`NO_DIRECTS`]: how to start one (the user's choice, 17 Nov 2026: the iPhone's).
pub const NO_DIRECTS_HINT: &str = "Start one from someone's card or the people list.";

/// Where a conversation shows while none is open (the user's choice, 17 Nov 2026). With
/// nothing joined, [`super::NOTHING_JOINED`] instead.
pub const PICK_A_CONVERSATION: &str = "Pick a conversation";

/// Where a conversation shows on a desktop with no server set yet.
pub const CHOOSE_A_SERVER: &str = "Choose a server in Settings to start.";

/// What saves what's typed, such as a server's fields.
pub const SAVE: &str = "Save";

/// What the system calls the phones' notifications of messages, one channel per
/// organisation grouped under its name (the user's choice, 17 Nov 2026).
pub const NOTIFICATION_CHANNEL: &str = "Messages";

/// How many of an event's paths show, one a line, before [`show_more_paths_label`] folds
/// the rest away until pressed (the user's decision, 11 Nov): a big push stays short.
pub const SHOWN_PATHS: usize = 8;

/// Where a search looks, on its scope's chip: `In #web`.
#[must_use]
pub fn scope_label(place: &str) -> String {
    format!("In {place}")
}

/// Where a match is, beside it: `in #web`.
#[must_use]
pub fn in_label(place: &str) -> String {
    format!("in {place}")
}

/// What shows in place of a shared file zirc.dev says was removed: `old.png was removed`.
/// Only that answer says so: a picture not had yet, or not reached, leaves the file as it is.
/// `name` as shown, its formatting stripped.
#[must_use]
pub fn removed_label(name: &str) -> String {
    format!("{name} was removed")
}

/// What an event's paths past [`SHOWN_PATHS`] say: `and 3 more`.
#[must_use]
pub fn more_paths_label(hidden: usize) -> String {
    format!("and {hidden} more")
}

/// What an event's folded paths past [`SHOWN_PATHS`] say on the button that shows them:
/// `Show 3 more`.
#[must_use]
pub fn show_more_paths_label(hidden: usize) -> String {
    format!("Show {hidden} more")
}

/// How many notifications are new, as a group of them sums up: `3 new`.
#[must_use]
pub fn new_label(count: usize) -> String {
    format!("{count} new")
}

/// How many mentions wait, as a status line or a screen reader says it: `1 mention`, `3
/// mentions`, capped as every badge is ([`super::badge_label`]): `99+ mentions`.
#[must_use]
pub fn mentions_label(count: usize) -> String {
    let noun = if count == 1 { "mention" } else { "mentions" };
    format!("{} {noun}", super::badge_label(count))
}

/// What a screen reader says of the chevron after a channel's name in its header, which
/// shows its people, threads and topic: `Details of #api` (the user's choice, 17 Nov 2026).
#[must_use]
pub fn details_label(title: &str) -> String {
    format!("Details of {title}")
}

/// What a screen reader says of what opens a person's or an agent's card, such as the
/// chevron after their name in a header or their face: `Profile of Sam`.
#[must_use]
pub fn profile_label(name: &str) -> String {
    format!("Profile of {name}")
}

/// What a screen reader says of the mark that folds a channel's guests' rooms under its row,
/// or shows them: `Show #api's guests` while folded, `Hide #api's guests` while open (the
/// user's choice, 17 Nov 2026).
#[must_use]
pub fn guests_label(title: &str, open: bool) -> String {
    let verb = if open { "Hide" } else { "Show" };
    format!("{verb} {title}'s guests")
}

/// The app's version as About shows it beside [`VERSION`]: `0.1.2`, and with the build
/// number where the app has one, `0.1.2 (25)` (the user's choice, 17 Nov 2026).
#[must_use]
pub fn version_label(version: &str, build: Option<&str>) -> String {
    match build.filter(|build| !build.is_empty()) {
        Some(build) => format!("{version} ({build})"),
        None => version.to_owned(),
    }
}

/// What Jump to says when nothing is called what's typed: `Nothing called “lunch”.`
#[must_use]
pub fn nothing_called(query: &str) -> String {
    format!("Nothing called “{}”.", query.trim())
}

/// What a sign-in that failed says: `Couldn't sign in: zirc.dev can't be reached`.
#[must_use]
pub fn signin_failed(why: &str) -> String {
    format!("{COULDNT_SIGN_IN}: {why}")
}

/// What a reaction's button says for a screen reader: `React with 👍`.
#[must_use]
pub fn react_with_label(emoji: &str) -> String {
    format!("React with {emoji}")
}

/// A reaction's chip as hovering over it and a screen reader say it in every app: the emoji
/// and how many gave it, as the chip shows them, then who: `👍 2, by bob, ann`.
#[must_use]
pub fn reacted_by_label(emoji: &str, nicks: &[String]) -> String {
    format!("{emoji} {}, by {}", nicks.len(), nicks.join(", "))
}

/// What's being shared, under the composer while it uploads: `Sharing notes.txt…`, `Sharing 3
/// files…`, and nothing while nothing is.
#[must_use]
pub fn sharing_label(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => format!("Sharing {one}…"),
        many => format!("Sharing {} files…", many.len()),
    }
}

/// What failed, under what was tried, such as a card's button: `Couldn't: timed out`.
#[must_use]
pub fn couldnt_label(why: &str) -> String {
    format!("Couldn't: {why}")
}

/// The words of a Mac's menu bar, in its title case: the Swift app's menus and the
/// desktop's say them alike. Elsewhere the same choices are said in sentence case, as
/// [`settings_list::ADD_ORGANISATION`] is.
pub mod menu_bar {
    /// Sparkle's check, under About.
    pub const CHECK_FOR_UPDATES: &str = "Check for Updates…";
    /// Adding an organisation, which opens zirc.dev's sign-in.
    pub const ADD_ORGANISATION: &str = "Add Organisation…";
    /// The conversation above the open one in the list.
    pub const PREVIOUS_CONVERSATION: &str = "Previous Conversation";
    /// The conversation below the open one in the list.
    pub const NEXT_CONVERSATION: &str = "Next Conversation";
    /// The menu of what's done to conversations ([`conversation`]): moving between them,
    /// finding one, and what's read.
    pub const CONVERSATION: &str = "Conversation";
    /// The unread conversation above the open one.
    pub const PREVIOUS_UNREAD: &str = "Previous Unread";
    /// The unread conversation below the open one.
    pub const NEXT_UNREAD: &str = "Next Unread";
    /// A direct message to someone not picked yet.
    pub const NEW_MESSAGE: &str = "New Message";
    /// The open conversation's details: its people, threads and topic, beside it.
    pub const DETAILS: &str = "Details";
    /// Marks the open conversation read.
    pub const MARK_READ: &str = "Mark Read";
    /// Marks every conversation read.
    pub const MARK_ALL_READ: &str = "Mark All Read";
    /// Search, in the conversation open.
    pub const SEARCH: &str = "Search…";
    /// Signs out of every organisation, after asking.
    pub const SIGN_OUT_EVERYTHING: &str = "Sign Out of Everything…";

    /// Which app's menu bar: each draws the items it can do.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum MenuApp {
        /// The GPUI desktop app.
        Desktop,
        /// The Swift Mac app.
        Mac,
    }

    /// What one of [`conversation`]'s items does.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Command {
        /// [`PREVIOUS_CONVERSATION`].
        PreviousConversation,
        /// [`NEXT_CONVERSATION`].
        NextConversation,
        /// [`PREVIOUS_UNREAD`].
        PreviousUnread,
        /// [`NEXT_UNREAD`].
        NextUnread,
        /// [`NEW_MESSAGE`].
        NewMessage,
        /// Jump to…, the quick switcher.
        JumpTo,
        /// [`DETAILS`].
        Details,
        /// [`MARK_READ`].
        MarkRead,
        /// [`MARK_ALL_READ`].
        MarkAllRead,
        /// [`SEARCH`].
        Search,
    }

    /// One item of the menu bar's [`CONVERSATION`] menu.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Item {
        /// What it does.
        pub command: Command,
        /// What it says.
        pub label: &'static str,
    }

    /// The [`CONVERSATION`] menu's items for `app`, in order (the user's choice, 17 Nov
    /// 2026): both desktops draw it from here, the same for each now the Mac starts a New
    /// Message too.
    #[must_use]
    pub fn conversation(_app: MenuApp) -> Vec<Item> {
        [
            (Command::PreviousConversation, PREVIOUS_CONVERSATION),
            (Command::NextConversation, NEXT_CONVERSATION),
            (Command::PreviousUnread, PREVIOUS_UNREAD),
            (Command::NextUnread, NEXT_UNREAD),
            (Command::NewMessage, NEW_MESSAGE),
            (Command::JumpTo, super::JUMP_TO_PROMPT),
            (Command::Details, DETAILS),
            (Command::MarkRead, MARK_READ),
            (Command::MarkAllRead, MARK_ALL_READ),
            (Command::Search, SEARCH),
        ]
        .into_iter()
        .map(|(command, label)| Item { command, label })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_each_phrase_as_every_app_does() {
        assert_eq!(scope_label("#web"), "In #web");
        assert_eq!(in_label("#web"), "in #web");
        assert_eq!(more_paths_label(3), "and 3 more");
        assert_eq!(show_more_paths_label(3), "Show 3 more");
        assert_eq!(new_label(3), "3 new");
        assert_eq!(removed_label("old.png"), "old.png was removed");
        assert_eq!(nothing_called(" lunch "), "Nothing called “lunch”.");
        assert_eq!(react_with_label("👍"), "React with 👍");
        assert_eq!(couldnt_label("timed out"), "Couldn't: timed out");
        assert_eq!(sharing_label(&[]), "");
        assert_eq!(
            sharing_label(&["notes.txt".to_owned()]),
            "Sharing notes.txt…"
        );
        assert_eq!(
            sharing_label(&["a.png".to_owned(), "b.png".to_owned()]),
            "Sharing 2 files…"
        );
        assert_eq!(mentions_label(1), "1 mention");
        assert_eq!(mentions_label(3), "3 mentions");
        assert_eq!(mentions_label(140), "99+ mentions");
        assert_eq!(details_label("#api"), "Details of #api");
        assert_eq!(profile_label("Sam"), "Profile of Sam");
        assert_eq!(guests_label("#api", false), "Show #api's guests");
        assert_eq!(guests_label("#api", true), "Hide #api's guests");
        assert_eq!(version_label("0.1.2", Some("25")), "0.1.2 (25)");
        assert_eq!(version_label("0.1.2", None), "0.1.2");
        assert_eq!(version_label("0.1.2", Some("")), "0.1.2");
        assert_eq!(signin_failed("timed out"), "Couldn't sign in: timed out");
        assert_eq!(
            activity_empty(),
            "Nothing yet. When someone mentions you, replies to you or reacts to what you said, it shows here."
        );
    }

    #[test]
    fn the_menu_bar_has_what_each_desktop_can_do() {
        use menu_bar::{MenuApp, conversation};
        let labels = |app| {
            conversation(app)
                .iter()
                .map(|item| item.label)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels(MenuApp::Desktop),
            [
                "Previous Conversation",
                "Next Conversation",
                "Previous Unread",
                "Next Unread",
                "New Message",
                "Jump to…",
                "Details",
                "Mark Read",
                "Mark All Read",
                "Search…",
            ]
        );
        assert_eq!(conversation(MenuApp::Mac), conversation(MenuApp::Desktop));
    }

    #[test]
    fn names_the_sidebar_parts_as_every_app_does() {
        use super::super::Shelf;
        let names = [Shelf::Starred, Shelf::Channels, Shelf::Direct].map(Shelf::label);
        assert_eq!(names, ["Starred", "Channels", "Direct messages"]);
        assert_eq!(BROWSE_CHANNELS, "Browse channels");
    }

    #[test]
    fn a_phrase_said_two_ways_is_one_phrase() {
        assert_eq!(JUMP_TO_PROMPT, format!("{JUMP_TO}…"));
        assert_eq!(
            settings_list::ADD_ORGANISATION,
            format!("{}…", settings_list::ADD_ORGANISATION_NAME)
        );
        assert!(MORE_PEOPLE_ONLY.starts_with(MORE));
        assert!(EARLIER_REPLY.ends_with(EARLIER_MESSAGE));
        assert!(COPY_MESSAGE_LINK.starts_with(COPY_LINK));
        assert_eq!(
            settings_list::SIGN_OUT_EVERYTHING,
            format!("{}…", crate::orgs::SIGN_OUT_EVERYTHING_NAME)
        );
        assert_eq!(
            menu_bar::ADD_ORGANISATION.to_lowercase(),
            settings_list::ADD_ORGANISATION.to_lowercase()
        );
    }
}

/// The core's fixed words an app shows as they are, handed over whole ([`words`]): a label
/// on a button, a heading, a pill. An app reads the field; it never types the word, which
/// zirc-app's `tests/words_guard.rs` holds every app to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct Words {
    /// A conversation's menu and its confirm button: `Leave`.
    pub leave: String,
    /// A sealed part's chip, to show its value: `Reveal`. `hide` hides it again.
    pub reveal: String,
    /// The composer's lock: `Seal (||…||)`.
    pub seal: String,
    /// What a direct message's menu says instead: `Close`, also what shuts a bar or a card.
    pub close: String,
    /// What an ended conversation's menu says: `Hide`.
    pub hide: String,
    /// `Star`.
    pub star: String,
    /// `Unstar`.
    pub unstar: String,
    /// `Mute`.
    pub mute: String,
    /// `Unmute`.
    pub unmute: String,
    /// `Mark as read`, in a row's menu and on a notification.
    pub mark_read: String,
    /// A card's button that opens a direct message: `Message`.
    pub message: String,
    /// The people list's heading, and what shows it: `People`.
    pub people: String,
    /// The heading over a conversation's apps: `Apps`.
    pub apps: String,
    /// The pill on an agent's name: `AGENT`.
    pub agent_tag: String,
    /// The pill on an app's name: `APP`.
    pub app_tag: String,
    /// While a search asks the history for more: `Searching…`.
    pub searching: String,
    /// What opens search with no conversation to look in: `Search everywhere`.
    pub search_everywhere: String,
    /// Browse channels' empty line: `No other channels to join`.
    pub no_other_channels: String,
    /// The page of what was said to the person, its tab and the bell that opens it:
    /// `Activity`.
    pub activity: String,
    /// Activity's empty page in one line: `nothing_yet`, a full stop and `activity_hint`.
    pub activity_empty: String,
    /// Under `nothing_yet` on Activity's empty page: `When someone mentions you, replies to you or
    /// reacts to what you said, it shows here.`
    pub activity_hint: String,
    /// The button that adds an organisation where it asks nothing first: `Add
    /// organisation`.
    pub add_organisation: String,
    /// What folds away a sign-in's server and connection: `Advanced`.
    pub advanced: String,
    /// While zirc.dev is asked: `Asking zirc.dev…`.
    pub asking_zirc_dev: String,
    /// What opens the emoji to react with, as a button's name: `Add reaction`.
    pub add_reaction: String,
    /// The composer's button that adds a file: `Attach`.
    pub attach: String,
    /// What goes back a step: `Back`.
    pub back: String,
    /// What a screen reader says of the `results` pill: `Back to search results`.
    pub back_to_results: String,
    /// What turns a question down, and stops what waits: `Cancel`.
    pub cancel: String,
    /// What empties a field: `Clear`.
    pub clear: String,
    /// What shuts search: `Close search`.
    pub close_search: String,
    /// The label of how a sign-in's connection stands: `Connection`.
    pub connection: String,
    /// A conversation's row's long press, for a screen reader: `Conversation options`.
    pub conversation_options: String,
    /// A code block's button that copies it: `Copy`.
    pub copy_block: String,
    /// A message's menu, copying a code block of it: `Copy code`.
    pub copy_code: String,
    /// A message's menu, copying a link it holds: `Copy link`.
    pub copy_link: String,
    /// A message's menu, copying a link to the message itself: `Copy link to message`.
    pub copy_message_link: String,
    /// A message's menu, copying what it says: `Copy text`.
    pub copy_text: String,
    /// A message's menu, showing what's attached to it, and the title of what that opens:
    /// `Message details`.
    pub message_details: String,
    /// A message's menu, deleting it: `Delete`.
    pub delete: String,
    /// Settings' section for deleting the account, and that page's title: `Delete
    /// account`.
    pub delete_account: String,
    /// The button that deletes the account: `Delete my account`.
    pub delete_my_account: String,
    /// What puts away an error once it's read: `Dismiss`.
    pub dismiss: String,
    /// What shuts a page once it's finished with: `Done`.
    pub done: String,
    /// Under `asking_zirc_dev` once a sign-in waits, before its code: `Check that the page in
    /// your browser shows`.
    pub check_the_code: String,
    /// New message's To: field's label: `To`.
    pub to: String,
    /// What New message's To: field says while it's empty: `Type a name`.
    pub type_a_name: String,
    /// What a video's player says while its copy is made: `Getting the video ready…`.
    pub video_making: String,
    /// What a video's player says when zirc.dev can't be reached: `Couldn't reach zirc.dev
    /// to play it`.
    pub video_unreachable: String,
    /// What a reply says of a message no longer kept: `a reply to an earlier message`.
    pub earlier_reply: String,
    /// What the composer's reply line names for a message no longer kept: `an earlier
    /// message`.
    pub earlier_message: String,
    /// Under `no_directs`: `Start one from someone's card or the people list.`
    pub no_directs_hint: String,
    /// Where a conversation shows while none is open: `Pick a conversation`.
    pub pick_a_conversation: String,
    /// Where a conversation shows with no server set: `Choose a server in Settings to start.`
    pub choose_a_server: String,
    /// What saves what's typed: `Save`.
    pub save: String,
    /// What the system calls the phones' notifications of messages: `Messages`.
    pub notification_channel: String,
    /// The menu bar's menu of conversations: `Conversation`.
    pub menu_conversation: String,
    /// Under the Away switch: `Others see you as away, and why, until you're back.`
    pub away_note: String,
    /// A file's button that fetches it: `Download`.
    pub download: String,
    /// A message's menu, correcting it: `Edit`.
    pub edit: String,
    /// What opens the person's own name and picture: `Edit profile`.
    pub edit_profile: String,
    /// After a corrected message's words, muted: `(edited)`.
    pub edited: String,
    /// What the composer says while it corrects a message, over its old words: `Editing`.
    pub editing: String,
    /// The button back to a conversation's end, for a screen reader: `Go to the newest`.
    pub go_to_newest: String,
    /// A guest's row of actions, for a screen reader: `Guest actions`.
    pub guest_actions: String,
    /// The first tab on a phone, and its page's title: `Home`.
    pub home: String,
    /// The quick switcher's name: `Jump to`.
    pub jump_to: String,
    /// What the quick switcher's empty field says, and what opens it: `Jump to…`.
    pub jump_to_prompt: String,
    /// A person's machines, as a heading on their card: `Machines`.
    pub machines: String,
    /// The menu bar's: `Add Organisation…`.
    pub menu_add_organisation: String,
    /// The menu bar's: `Check for Updates…`.
    pub menu_check_for_updates: String,
    /// The menu bar's: `Next Conversation`.
    pub menu_next_conversation: String,
    /// The menu bar's: `Previous Conversation`.
    pub menu_previous_conversation: String,
    /// The button that opens a menu of what else can be done: `More`.
    pub more: String,
    /// A header's `more` for a screen reader while `people_only` is on: `More, showing
    /// people only`.
    pub more_people_only: String,
    /// What starts a direct message to someone not picked yet: `New message`.
    pub new_message: String,
    /// What marks the first message unread when a conversation was opened, on a tag: `NEW`.
    pub new_tag: String,
    /// The channel list's empty line: `No channels yet.`.
    pub no_channels: String,
    /// Direct messages' empty line: `No direct messages yet`.
    pub no_directs: String,
    /// What the emoji picker says when nothing matches: `No emoji match.`.
    pub no_emoji: String,
    /// What puts a question off: `Not now`.
    pub not_now: String,
    /// Activity's empty line: `Nothing yet`.
    pub nothing_yet: String,
    /// The organisations signed in to, as a list's title and a menu's: `Organisations`.
    pub organisations: String,
    /// A header's menu, showing only what people said: `People only`.
    pub people_only: String,
    /// The title of the person's name and picture: `Profile`.
    pub profile: String,
    /// What opens the emoji to react to a message with: `React`.
    pub react: String,
    /// What answers a message: `Reply`.
    pub reply: String,
    /// A search's matches, and the pill that goes back to them: `Results`.
    pub results: String,
    /// What tries to connect again from the offline line: `Retry`.
    pub retry: String,
    /// The title of the list of people a search narrows to: `Said by`.
    pub said_by: String,
    /// Search's name, on its page and what opens it: `Search`.
    pub search: String,
    /// The emoji picker's field while it's empty: `Search emoji`.
    pub search_emoji: String,
    /// The title of where a search looks: `Search in`.
    pub search_in: String,
    /// The button that sends what's written: `Send`.
    pub send: String,
    /// While a card's button is being pressed: `Sending…`.
    pub sending: String,
    /// The label of the server a sign-in is to: `Server`.
    pub server: String,
    /// Settings' own name, on what opens it: `Settings`.
    pub settings: String,
    /// What opens the settings no one organisation owns: `Settings for all
    /// organisations`.
    pub settings_all: String,
    /// What signs out, before the core says of what: `Sign out`.
    pub sign_out: String,
    /// The label of who a sign-in is: `Signed in`.
    pub signed_in: String,
    /// What shuts the composer's editing line: `Stop editing`.
    pub stop_editing: String,
    /// What shuts the composer's reply line: `Stop replying`.
    pub stop_replying: String,
    /// A channel's threads, the list and what shows it: `Threads`.
    pub threads: String,
    /// What sends again what failed: `Try again`.
    pub try_again: String,
    /// What asks to let the app notify: `Turn on notifications`.
    pub turn_on_notifications: String,
    /// The label of the app's version: `Version`.
    pub version: String,
    /// The person's own page, its tab and their row: `You`.
    pub you: String,
    /// What stands for a name not given yet: `Your name`.
    pub your_name: String,
    /// While what's drawn next isn't here yet: `Loading…`.
    pub loading: String,
    /// While a sign-in finishes: `Signing in…`.
    pub signing_in: String,
    /// The button that connects with the server and nick typed in: `Connect`.
    pub connect: String,
    /// The title over a sign-in with no organisation: `No organisation yet`.
    pub no_organisation: String,
    /// Under `no_organisation`, before `create_or_join` and `then_come_back`.
    pub no_organisation_note: String,
    /// `no_organisation_note`'s link: `Create or join one`.
    pub create_or_join: String,
    /// What follows `create_or_join`: `, then come back.`.
    pub then_come_back: String,
    /// The title over a failed sign-in: `Couldn't sign in`.
    pub couldnt_sign_in: String,
    /// Why a guest can't press a card's button.
    pub card_needs_member: String,
    /// Why what needs the server can't be done now: `Not connected.`.
    pub not_connected: String,
    /// A guest's own room's note, for a screen reader: `Private`.
    pub room_private: String,
    /// A guest's own room's note.
    pub room_private_note: String,
    /// A public room's note, for a screen reader: `Public`.
    pub room_public: String,
    /// A public room's note.
    pub room_public_note: String,
    /// The line over the first unread message, for a screen reader: `New messages`.
    pub new_messages: String,
    /// The note over a composer writing to a guest, for a screen reader.
    pub writing_to_guest: String,
    /// The line of who's typing, for a screen reader: `Typing`.
    pub typing: String,
    /// What shuts the people list: `Close people`.
    pub close_people: String,
    /// What shuts a person's card: `Close profile`.
    pub close_profile: String,
    /// A card's list of what others change too, for a screen reader.
    pub both_changing: String,
    /// A message's hover bar, for a screen reader: `Message actions`.
    pub message_actions: String,
    /// A card's buttons, for a screen reader: `Card actions`.
    pub card_actions: String,
    /// What a copy button says once it has copied: `Copied`.
    pub copied: String,
    /// The picture or file shown large, for a screen reader: `Viewer`.
    pub viewer: String,
    /// What shuts `viewer`: `Close viewer`.
    pub close_viewer: String,
    /// What opens what `viewer` shows in a tab: `Open in a new tab`.
    pub open_in_new_tab: String,
    /// When a guest page's captcha can't load.
    pub captcha_failed: String,
    /// What signs in, as a link: `Sign in`.
    pub sign_in: String,
    /// After `sign_in` on a guest's page.
    pub to_chat_as_yourself: String,
    /// A guest's name field while it's empty.
    pub guest_name_hint: String,
    /// A guest's name field's label: `Your name (optional)`.
    pub your_name_optional: String,
    /// While a guest joins: `Joining…`.
    pub joining: String,
    /// What a guest joins with: `Start chatting`.
    pub start_chatting: String,
    /// The composer's reply line, for a screen reader: `Replying`.
    pub replying: String,
    /// The composer's completions, for a screen reader: `Suggestions`.
    pub suggestions: String,
    /// The emoji picker, and the button that opens it: `Emoji`.
    pub emoji: String,
    /// The search field, for a screen reader: `Search messages`.
    pub search_messages: String,
    /// A search's filters, for a screen reader: `Filters`.
    pub filters: String,
    /// Who a guest's conversation can be assigned to: `Assign to`.
    pub assign_to: String,
    /// What opens what else an admin can do to a guest.
    pub more_guest_actions: String,
    /// The list of conversations, for a screen reader: `Conversations`.
    pub conversations: String,
    /// The sidebar's tabs, for a screen reader: `Sidebar`.
    pub sidebar: String,
    /// The list of messages, for a screen reader: `Messages`.
    pub messages: String,
    /// The dot on a guest's conversation assigned to the person.
    pub assigned_to_you: String,
    /// When a file's upload fails.
    pub upload_failed: String,
    /// When zirc.dev's web app can't be reached.
    pub web_app_unreachable: String,
    /// The person's account, as a menu's name: `Account`.
    pub account: String,
    /// Said beside what Esc closes: `Esc closes`.
    pub esc_closes: String,
    /// A sign-in's title: `Sign in to Zirc`.
    pub sign_in_to_zirc: String,
    /// The button that signs in in the browser: `Sign in with zirc.dev`.
    pub sign_in_with_zirc_dev: String,
    /// The button that asks who a guest's conversation goes to: `Assign to…`.
    pub assign_to_label: String,
    /// A message's menu item that opens the emoji to react with: `React…`.
    pub react_prompt: String,
    /// The menu bar's: `Sign Out of Everything…`.
    pub menu_sign_out_everything: String,
    /// What a screen reader says of a conversation's muted mark: `muted`.
    pub muted: String,
    /// The mark before a reply's quote: `↪`.
    pub quote_mark: String,
    /// The message field and the empty pane before it's ever connected: `Connecting…`.
    pub connecting: String,
    /// Jump to's key, beside its field: `⌘K`.
    pub jump_key: String,
    /// Search everywhere's key, beside its row: `⌘⇧F`.
    pub search_everywhere_key: String,
    /// A phone's message menu's item after Copy text: `Select text`.
    pub select_text: String,
    /// The composer's attach menu, picking a photo: `Photo`.
    pub photo: String,
    /// The composer's attach menu, picking a file: `File`.
    pub file: String,
    /// What opens an agent session's card from its row's menu: `Card`.
    pub card: String,
    /// What forgets the server a sign-in was set to: `Stop using this server`.
    pub stop_using_server: String,
    /// What the organisation's tile does when pressed: `Switch organisation`.
    pub switch_organisation: String,
    /// What a screen reader says of the tile while another organisation mentions the
    /// person: `Mentions in another organisation`.
    pub mentions_elsewhere: String,
}

/// The core's fixed words ([`Words`]).
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one field a line, each a constant above: split, it would only be harder to read"
)]
pub fn words() -> Words {
    Words {
        leave: menu::LEAVE.to_owned(),
        reveal: super::sealing::REVEAL.to_owned(),
        seal: super::sealing::SEAL.to_owned(),
        close: menu::CLOSE.to_owned(),
        hide: menu::HIDE.to_owned(),
        star: menu::STAR.to_owned(),
        unstar: menu::UNSTAR.to_owned(),
        mute: menu::MUTE.to_owned(),
        unmute: menu::UNMUTE.to_owned(),
        mark_read: menu::MARK_READ.to_owned(),
        message: agent_card::MESSAGE.to_owned(),
        people: plain::PEOPLE_SECTION.to_owned(),
        apps: plain::APPS_SECTION.to_owned(),
        agent_tag: speaker::AGENT_TAG.to_owned(),
        app_tag: speaker::APP_TAG.to_owned(),
        searching: search::SEARCHING_OLDER.to_owned(),
        search_everywhere: finder::SEARCH_EVERYWHERE.to_owned(),
        no_other_channels: plain::NO_OTHER_CHANNELS.to_owned(),
        activity: ACTIVITY.to_owned(),
        activity_empty: activity_empty(),
        activity_hint: ACTIVITY_HINT.to_owned(),
        add_organisation: settings_list::ADD_ORGANISATION_NAME.to_owned(),
        advanced: ADVANCED.to_owned(),
        asking_zirc_dev: ASKING_ZIRC_DEV.to_owned(),
        add_reaction: ADD_REACTION.to_owned(),
        attach: ATTACH.to_owned(),
        back: BACK.to_owned(),
        back_to_results: BACK_TO_RESULTS.to_owned(),
        cancel: CANCEL.to_owned(),
        clear: CLEAR.to_owned(),
        close_search: CLOSE_SEARCH.to_owned(),
        connection: CONNECTION.to_owned(),
        conversation_options: CONVERSATION_OPTIONS.to_owned(),
        copy_block: COPY.to_owned(),
        copy_code: COPY_CODE.to_owned(),
        copy_link: COPY_LINK.to_owned(),
        copy_message_link: COPY_MESSAGE_LINK.to_owned(),
        copy_text: COPY_TEXT.to_owned(),
        message_details: super::MESSAGE_DETAILS.to_owned(),
        delete: DELETE.to_owned(),
        delete_account: settings_list::DELETE_ACCOUNT.to_owned(),
        delete_my_account: DELETE_MY_ACCOUNT.to_owned(),
        dismiss: DISMISS.to_owned(),
        done: DONE.to_owned(),
        check_the_code: CHECK_THE_CODE.to_owned(),
        to: super::TO.to_owned(),
        type_a_name: super::TYPE_A_NAME.to_owned(),
        video_making: super::VIDEO_MAKING.to_owned(),
        video_unreachable: super::VIDEO_UNREACHABLE.to_owned(),
        earlier_reply: EARLIER_REPLY.to_owned(),
        earlier_message: EARLIER_MESSAGE.to_owned(),
        no_directs_hint: NO_DIRECTS_HINT.to_owned(),
        pick_a_conversation: PICK_A_CONVERSATION.to_owned(),
        choose_a_server: CHOOSE_A_SERVER.to_owned(),
        save: SAVE.to_owned(),
        notification_channel: NOTIFICATION_CHANNEL.to_owned(),
        menu_conversation: menu_bar::CONVERSATION.to_owned(),
        away_note: settings_list::AWAY_NOTE.to_owned(),
        download: DOWNLOAD.to_owned(),
        edit: super::EDIT_LABEL.to_owned(),
        edit_profile: EDIT_PROFILE.to_owned(),
        edited: super::EDITED.to_owned(),
        editing: super::EDITING.to_owned(),
        go_to_newest: GO_TO_NEWEST.to_owned(),
        guest_actions: GUEST_ACTIONS.to_owned(),
        home: HOME.to_owned(),
        jump_to: JUMP_TO.to_owned(),
        jump_to_prompt: JUMP_TO_PROMPT.to_owned(),
        machines: MACHINES.to_owned(),
        menu_add_organisation: menu_bar::ADD_ORGANISATION.to_owned(),
        menu_check_for_updates: menu_bar::CHECK_FOR_UPDATES.to_owned(),
        menu_next_conversation: menu_bar::NEXT_CONVERSATION.to_owned(),
        menu_previous_conversation: menu_bar::PREVIOUS_CONVERSATION.to_owned(),
        more: MORE.to_owned(),
        more_people_only: MORE_PEOPLE_ONLY.to_owned(),
        new_message: NEW_MESSAGE.to_owned(),
        new_tag: NEW_TAG.to_owned(),
        no_channels: NO_CHANNELS.to_owned(),
        no_directs: NO_DIRECTS.to_owned(),
        no_emoji: crate::emoji::NO_MATCH.to_owned(),
        not_now: NOT_NOW.to_owned(),
        nothing_yet: NOTHING_YET.to_owned(),
        organisations: settings_list::ORGANISATIONS.to_owned(),
        people_only: PEOPLE_ONLY.to_owned(),
        profile: PROFILE.to_owned(),
        react: REACT_LABEL.to_owned(),
        reply: REPLY_LABEL.to_owned(),
        results: RESULTS.to_owned(),
        retry: RETRY.to_owned(),
        said_by: SAID_BY.to_owned(),
        search: SEARCH.to_owned(),
        search_emoji: crate::emoji::SEARCH.to_owned(),
        search_in: SEARCH_IN.to_owned(),
        send: SEND.to_owned(),
        sending: SENDING.to_owned(),
        server: settings_list::SERVER.to_owned(),
        settings: settings_list::SETTINGS.to_owned(),
        settings_all: SETTINGS_ALL.to_owned(),
        sign_out: crate::orgs::SIGN_OUT.to_owned(),
        signed_in: SIGNED_IN.to_owned(),
        stop_editing: super::STOP_EDITING.to_owned(),
        stop_replying: STOP_REPLYING.to_owned(),
        threads: THREADS.to_owned(),
        try_again: TRY_AGAIN.to_owned(),
        turn_on_notifications: TURN_ON_NOTIFICATIONS.to_owned(),
        version: VERSION.to_owned(),
        you: YOU_PAGE.to_owned(),
        your_name: YOUR_NAME.to_owned(),
        loading: labels::LOADING.to_owned(),
        signing_in: labels::SIGNING_IN.to_owned(),
        connect: labels::CONNECT.to_owned(),
        no_organisation: labels::NO_ORGANISATION.to_owned(),
        no_organisation_note: labels::NO_ORGANISATION_NOTE.to_owned(),
        create_or_join: labels::CREATE_OR_JOIN.to_owned(),
        then_come_back: labels::THEN_COME_BACK.to_owned(),
        couldnt_sign_in: super::COULDNT_SIGN_IN.to_owned(),
        card_needs_member: labels::CARD_NEEDS_MEMBER.to_owned(),
        not_connected: labels::NOT_CONNECTED.to_owned(),
        room_private: labels::ROOM_PRIVATE.to_owned(),
        room_private_note: labels::ROOM_PRIVATE_NOTE.to_owned(),
        room_public: labels::ROOM_PUBLIC.to_owned(),
        room_public_note: labels::ROOM_PUBLIC_NOTE.to_owned(),
        new_messages: labels::NEW_MESSAGES.to_owned(),
        writing_to_guest: labels::WRITING_TO_GUEST.to_owned(),
        typing: labels::TYPING.to_owned(),
        close_people: labels::CLOSE_PEOPLE.to_owned(),
        close_profile: labels::CLOSE_PROFILE.to_owned(),
        both_changing: labels::BOTH_CHANGING.to_owned(),
        message_actions: labels::MESSAGE_ACTIONS.to_owned(),
        card_actions: labels::CARD_ACTIONS.to_owned(),
        copied: labels::COPIED.to_owned(),
        viewer: labels::VIEWER.to_owned(),
        close_viewer: labels::CLOSE_VIEWER.to_owned(),
        open_in_new_tab: labels::OPEN_IN_NEW_TAB.to_owned(),
        captcha_failed: labels::CAPTCHA_FAILED.to_owned(),
        sign_in: labels::SIGN_IN.to_owned(),
        to_chat_as_yourself: labels::TO_CHAT_AS_YOURSELF.to_owned(),
        guest_name_hint: labels::GUEST_NAME_HINT.to_owned(),
        your_name_optional: labels::your_name_optional(),
        joining: labels::JOINING.to_owned(),
        start_chatting: labels::START_CHATTING.to_owned(),
        replying: labels::REPLYING.to_owned(),
        suggestions: labels::SUGGESTIONS.to_owned(),
        emoji: labels::EMOJI.to_owned(),
        search_messages: labels::SEARCH_MESSAGES.to_owned(),
        filters: labels::FILTERS.to_owned(),
        assign_to: labels::ASSIGN_TO.to_owned(),
        more_guest_actions: labels::MORE_GUEST_ACTIONS.to_owned(),
        conversations: labels::CONVERSATIONS.to_owned(),
        sidebar: labels::SIDEBAR.to_owned(),
        messages: labels::MESSAGES.to_owned(),
        assigned_to_you: labels::ASSIGNED_TO_YOU.to_owned(),
        upload_failed: labels::UPLOAD_FAILED.to_owned(),
        web_app_unreachable: labels::WEB_APP_UNREACHABLE.to_owned(),
        account: settings_list::SETTINGS_ACCOUNT.to_owned(),
        esc_closes: labels::ESC_CLOSES.to_owned(),
        sign_in_to_zirc: labels::sign_in_to_zirc(),
        sign_in_with_zirc_dev: labels::SIGN_IN_WITH_ZIRC_DEV.to_owned(),
        assign_to_label: labels::ASSIGN_TO_LABEL.to_owned(),
        react_prompt: labels::REACT_PROMPT.to_owned(),
        menu_sign_out_everything: menu_bar::SIGN_OUT_EVERYTHING.to_owned(),
        muted: labels::MUTED.to_owned(),
        quote_mark: plain::QUOTE_MARK.to_owned(),
        connecting: crate::connection::CONNECTING.to_owned(),
        jump_key: labels::keys::JUMP_TO.to_owned(),
        search_everywhere_key: labels::keys::SEARCH_EVERYWHERE.to_owned(),
        select_text: labels::SELECT_TEXT.to_owned(),
        photo: labels::PHOTO.to_owned(),
        file: labels::FILE.to_owned(),
        card: labels::CARD.to_owned(),
        stop_using_server: labels::STOP_USING_SERVER.to_owned(),
        switch_organisation: labels::SWITCH_ORGANISATION.to_owned(),
        mentions_elsewhere: labels::MENTIONS_ELSEWHERE.to_owned(),
    }
}
