//! Small phrases every app says the same way, said once here: what the composer says
//! while replying, where a search looks and where a match is, how many paths an event
//! leaves out or folds away, how many notifications are new, what Jump to says when nothing matches, and
//! the sidebar's names that aren't a [`Shelf`](super::Shelf)'s. [`Words`] hands the ones
//! that take no argument to an app whole, so it types none of them.

use serde::Serialize;
use ts_rs::TS;

use super::{Shelf, agent_card, finder, notify::menu, plain, search, settings_list, speaker};

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

/// Direct messages where room is short, such as a tab bar: a heading says
/// [`Shelf::Direct`](super::Shelf::Direct)'s label in full.
pub const DIRECT_SHORT: &str = "DMs";

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

/// The composer's button that adds a file.
pub const ATTACH: &str = "Attach";

/// What goes back a step: out of a question a card's button asks, out of a list.
pub const BACK: &str = "Back";

/// What a screen reader says of [`RESULTS`]' pill.
pub const BACK_TO_RESULTS: &str = "Back to search results";

/// What turns a question down, and stops what waits.
pub const CANCEL: &str = "Cancel";

/// What asks before changing something, such as a nickname.
pub const CHANGE: &str = "Change…";

/// What empties a field, for a screen reader.
pub const CLEAR: &str = "Clear";

/// What shuts search, for a screen reader.
pub const CLOSE_SEARCH: &str = "Close search";

/// The label of how a sign-in's connection stands, under [`ADVANCED`].
pub const CONNECTION: &str = "Connection";

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

/// What opens zirc.dev's page for inviting people: its row in Settings, an owner's or an
/// admin's alone.
pub const INVITE_PEOPLE: &str = "Invite people";

/// The quick switcher's name, for a screen reader.
pub const JUMP_TO: &str = "Jump to";

/// What the quick switcher's empty field says, and what opens it.
pub const JUMP_TO_PROMPT: &str = "Jump to…";

/// The button that opens a menu of what else can be done, on a message and in a header.
pub const MORE: &str = "More";

/// What a screen reader says of a header's [`MORE`] while [`PEOPLE_ONLY`] is on.
pub const MORE_PEOPLE_ONLY: &str = "More, showing people only";

/// The label of the name the person shows as.
pub const NAME: &str = "Name";

/// What marks the first message unread when a conversation was opened.
pub const NEW_DIVIDER: &str = "New";

/// The heading over what an organisation joined from now on shows of the person.
pub const NEW_ORGANISATIONS: &str = "New organisations";

/// [`NEW_DIVIDER`] where a tag is drawn in capitals without a style to raise it.
pub const NEW_TAG: &str = "NEW";

/// The label of the person's nickname, the handle after the @.
pub const NICKNAME: &str = "Nickname";

/// Under the nickname's field: what a nickname is.
pub const NICKNAME_NOTE: &str = "Your name on the server, which people @.";

/// [`ACTIVITY`]'s empty line.
pub const NOTHING_YET: &str = "Nothing yet";

/// Machines' empty line.
pub const NO_AGENTS: &str = "No agents at work";

/// The channel list's empty line, once connected.
pub const NO_CHANNELS: &str = "No channels yet.";

/// Direct messages' empty line.
pub const NO_DIRECTS: &str = "No direct messages yet";

/// A header's menu: shows only what people said.
pub const PEOPLE_ONLY: &str = "People only";

/// The heading over the person's picture.
pub const PICTURE: &str = "Picture";

/// The title of the person's name and picture, while it loads too.
pub const PROFILE: &str = "Profile";

/// What opens the emoji to react to a message with.
pub const REACT_LABEL: &str = "React";

/// What answers a message: in its menu, on its hover bar, on a notification.
pub const REPLY_LABEL: &str = "Reply";

/// The list of a search's matches, and the pill in a conversation that goes back to it.
pub const RESULTS: &str = "Results";

/// On zircd's notice that an agent's wakes are paused: the button that lets the held ones
/// through.
pub const RESUME: &str = "Resume";

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

/// The label of the app's version, in About.
pub const VERSION: &str = "Version";

/// What a name's field says before one is given, and what a screen reader calls the note
/// of the name a guest got. The web page types it too on its guest form, drawn before its
/// wasm is loaded (`oss/web/src/guest.tsx`).
pub const YOUR_NAME: &str = "Your name";

/// The person's own page and its tab, and the row that stands for them. A preview's
/// speaker is [`Preview`](super::Preview)'s own.
pub const YOU_PAGE: &str = "You";

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

/// What Jump to says when nothing is called what's typed: `Nothing called “lunch”.`
#[must_use]
pub fn nothing_called(query: &str) -> String {
    format!("Nothing called “{}”.", query.trim())
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
        assert_eq!(
            activity_empty(),
            "Nothing yet. When someone mentions you, replies to you or reacts to what you said, it shows here."
        );
    }

    #[test]
    fn names_the_sidebar_parts_as_every_app_does() {
        use super::super::Shelf;
        let names = [
            Shelf::Starred,
            Shelf::Channels,
            Shelf::Direct,
            Shelf::Machines,
        ]
        .map(Shelf::label);
        assert_eq!(
            names,
            ["Starred", "Channels", "Direct messages", "Machines"]
        );
        assert_eq!(BROWSE_CHANNELS, "Browse channels");
        assert_eq!(DIRECT_SHORT, "DMs");
    }

    #[test]
    fn a_phrase_said_two_ways_is_one_phrase() {
        assert_eq!(NEW_TAG, NEW_DIVIDER.to_uppercase());
        assert_eq!(JUMP_TO_PROMPT, format!("{JUMP_TO}…"));
        assert_eq!(
            settings_list::ADD_ORGANISATION,
            format!("{}…", settings_list::ADD_ORGANISATION_NAME)
        );
        assert_eq!(
            settings_list::CHANGE_PICTURE,
            format!("{}…", settings_list::CHANGE_PICTURE_NAME)
        );
        assert!(MORE_PEOPLE_ONLY.starts_with(MORE));
        assert!(COPY_MESSAGE_LINK.starts_with(COPY_LINK));
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
    /// The composer's button that adds a file: `Attach`.
    pub attach: String,
    /// What goes back a step: `Back`.
    pub back: String,
    /// What a screen reader says of the `results` pill: `Back to search results`.
    pub back_to_results: String,
    /// What turns a question down, and stops what waits: `Cancel`.
    pub cancel: String,
    /// What asks before changing something, such as a nickname: `Change…`.
    pub change: String,
    /// The button that picks another picture, in the profile: `Change picture`.
    pub change_picture: String,
    /// What empties a field: `Clear`.
    pub clear: String,
    /// What shuts search: `Close search`.
    pub close_search: String,
    /// The label of how a sign-in's connection stands: `Connection`.
    pub connection: String,
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
    /// A message's menu, deleting it: `Delete`.
    pub delete: String,
    /// Settings' section for deleting the account, and that page's title: `Delete
    /// account`.
    pub delete_account: String,
    /// The button that deletes the account: `Delete my account`.
    pub delete_my_account: String,
    /// What shuts a page once it's finished with: `Done`.
    pub done: String,
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
    /// What opens the page for inviting people: `Invite people`.
    pub invite_people: String,
    /// The quick switcher's name: `Jump to`.
    pub jump_to: String,
    /// What the quick switcher's empty field says, and what opens it: `Jump to…`.
    pub jump_to_prompt: String,
    /// A person's machines, as a heading on their card: `Machines`, the sidebar's name
    /// for them.
    pub machines: String,
    /// The button that opens a menu of what else can be done: `More`.
    pub more: String,
    /// A header's `more` for a screen reader while `people_only` is on: `More, showing
    /// people only`.
    pub more_people_only: String,
    /// The label of the name the person shows as: `Name`.
    pub name: String,
    /// What marks the first message unread when a conversation was opened: `New`.
    pub new_divider: String,
    /// The heading over what organisations joined from now on show: `New organisations`.
    pub new_organisations: String,
    /// `new_divider` where a tag is drawn in capitals: `NEW`.
    pub new_tag: String,
    /// The label of the person's nickname: `Nickname`.
    pub nickname: String,
    /// Under the nickname's field: `Your name on the server, which people @.`
    pub nickname_note: String,
    /// Machines' empty line: `No agents at work`.
    pub no_agents: String,
    /// The channel list's empty line: `No channels yet.`.
    pub no_channels: String,
    /// Direct messages' empty line: `No direct messages yet`.
    pub no_directs: String,
    /// What the emoji picker says when nothing matches: `No emoji match.`.
    pub no_emoji: String,
    /// Activity's empty line: `Nothing yet`.
    pub nothing_yet: String,
    /// The organisations signed in to, as a list's title and a menu's: `Organisations`.
    pub organisations: String,
    /// A header's menu, showing only what people said: `People only`.
    pub people_only: String,
    /// The heading over the person's picture: `Picture`.
    pub picture: String,
    /// The title of the person's name and picture: `Profile`.
    pub profile: String,
    /// What opens the emoji to react to a message with: `React`.
    pub react: String,
    /// What answers a message: `Reply`.
    pub reply: String,
    /// A search's matches, and the pill that goes back to them: `Results`.
    pub results: String,
    /// A paused-wakes notice's button that lets the held ones through: `Resume`.
    pub resume: String,
    /// What tries to connect again from the offline line: `Retry`.
    pub retry: String,
    /// The title of the list of people a search narrows to: `Said by`.
    pub said_by: String,
    /// Search's name, on its page and what opens it: `Search`.
    pub search: String,
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
    /// The label of the app's version: `Version`.
    pub version: String,
    /// The person's own page, its tab and their row: `You`.
    pub you: String,
    /// What stands for a name not given yet: `Your name`.
    pub your_name: String,
}

/// The core's fixed words ([`Words`]).
#[must_use]
pub fn words() -> Words {
    Words {
        leave: menu::LEAVE.to_owned(),
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
        attach: ATTACH.to_owned(),
        back: BACK.to_owned(),
        back_to_results: BACK_TO_RESULTS.to_owned(),
        cancel: CANCEL.to_owned(),
        change: CHANGE.to_owned(),
        change_picture: settings_list::CHANGE_PICTURE_NAME.to_owned(),
        clear: CLEAR.to_owned(),
        close_search: CLOSE_SEARCH.to_owned(),
        connection: CONNECTION.to_owned(),
        copy_block: COPY.to_owned(),
        copy_code: COPY_CODE.to_owned(),
        copy_link: COPY_LINK.to_owned(),
        copy_message_link: COPY_MESSAGE_LINK.to_owned(),
        copy_text: COPY_TEXT.to_owned(),
        delete: DELETE.to_owned(),
        delete_account: settings_list::DELETE_ACCOUNT.to_owned(),
        delete_my_account: DELETE_MY_ACCOUNT.to_owned(),
        done: DONE.to_owned(),
        download: DOWNLOAD.to_owned(),
        edit: super::EDIT_LABEL.to_owned(),
        edit_profile: EDIT_PROFILE.to_owned(),
        edited: super::EDITED.to_owned(),
        editing: super::EDITING.to_owned(),
        go_to_newest: GO_TO_NEWEST.to_owned(),
        guest_actions: GUEST_ACTIONS.to_owned(),
        home: HOME.to_owned(),
        invite_people: INVITE_PEOPLE.to_owned(),
        jump_to: JUMP_TO.to_owned(),
        jump_to_prompt: JUMP_TO_PROMPT.to_owned(),
        machines: Shelf::Machines.label().to_owned(),
        more: MORE.to_owned(),
        more_people_only: MORE_PEOPLE_ONLY.to_owned(),
        name: NAME.to_owned(),
        new_divider: NEW_DIVIDER.to_owned(),
        new_organisations: NEW_ORGANISATIONS.to_owned(),
        new_tag: NEW_TAG.to_owned(),
        nickname: NICKNAME.to_owned(),
        nickname_note: NICKNAME_NOTE.to_owned(),
        no_agents: NO_AGENTS.to_owned(),
        no_channels: NO_CHANNELS.to_owned(),
        no_directs: NO_DIRECTS.to_owned(),
        no_emoji: crate::emoji::NO_MATCH.to_owned(),
        nothing_yet: NOTHING_YET.to_owned(),
        organisations: settings_list::ORGANISATIONS.to_owned(),
        people_only: PEOPLE_ONLY.to_owned(),
        picture: PICTURE.to_owned(),
        profile: PROFILE.to_owned(),
        react: REACT_LABEL.to_owned(),
        reply: REPLY_LABEL.to_owned(),
        results: RESULTS.to_owned(),
        resume: RESUME.to_owned(),
        retry: RETRY.to_owned(),
        said_by: SAID_BY.to_owned(),
        search: SEARCH.to_owned(),
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
        version: VERSION.to_owned(),
        you: YOU_PAGE.to_owned(),
        your_name: YOUR_NAME.to_owned(),
    }
}
