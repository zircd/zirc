//! Labels the web chat typed until 20 Nov 2026, said once here so every app says them
//! alike: what a screen reader calls a part of the page, what a sign-in says while it
//! waits, a public room's notes. [`super::Words`] hands them over; the functions take what
//! goes in them.

/// While what's drawn next isn't here yet.
pub const LOADING: &str = "Loading…";

/// While a sign-in finishes.
pub const SIGNING_IN: &str = "Signing in…";

/// The button that connects with the server and nick typed in.
pub const CONNECT: &str = "Connect";

/// The title over a sign-in to zirc.dev with no organisation.
pub const NO_ORGANISATION: &str = "No organisation yet";

/// Under [`NO_ORGANISATION`], before [`CREATE_OR_JOIN`] (a link to zirc.dev's
/// organisations) and [`THEN_COME_BACK`].
pub const NO_ORGANISATION_NOTE: &str = "You're not in an organisation on zirc.dev yet.";

/// [`NO_ORGANISATION_NOTE`]'s link.
pub const CREATE_OR_JOIN: &str = "Create or join one";

/// What follows [`CREATE_OR_JOIN`].
pub const THEN_COME_BACK: &str = ", then come back.";

/// Why a card's button can't be pressed by a guest.
pub const CARD_NEEDS_MEMBER: &str = "Only a member signed in to the organisation can press it.";

/// Why something that needs the server can't be done now.
pub const NOT_CONNECTED: &str = "Not connected.";

/// What a screen reader calls a guest's room's note, in their own room.
pub const ROOM_PRIVATE: &str = "Private";

/// A guest's own room's note: who reads it.
pub const ROOM_PRIVATE_NOTE: &str =
    "Only you and the team see what you write here. Please don't share passwords.";

/// What a screen reader calls a public room's note.
pub const ROOM_PUBLIC: &str = "Public";

/// A public room's note: who reads it.
pub const ROOM_PUBLIC_NOTE: &str = "Everyone in this room sees what you write here, so please don't share passwords or private details.";

/// What a screen reader calls the line over the first unread message, and the bar that
/// jumps to it.
pub const NEW_MESSAGES: &str = "New messages";

/// What a screen reader calls the note over a composer that writes to a guest.
pub const WRITING_TO_GUEST: &str = "Writing to a guest";

/// What a screen reader calls the line of who's typing.
pub const TYPING: &str = "Typing";

/// What shuts the people list.
pub const CLOSE_PEOPLE: &str = "Close people";

/// What shuts a person's card.
pub const CLOSE_PROFILE: &str = "Close profile";

/// What a screen reader calls a card's list of what others change too.
pub const BOTH_CHANGING: &str = "Also being changed by others";

/// What a screen reader calls a message's hover bar.
pub const MESSAGE_ACTIONS: &str = "Message actions";

/// What a screen reader calls a card's buttons.
pub const CARD_ACTIONS: &str = "Card actions";

/// What a copy button says once it has copied.
pub const COPIED: &str = "Copied";

/// What a screen reader calls the picture or file shown large.
pub const VIEWER: &str = "Viewer";

/// What shuts [`VIEWER`].
pub const CLOSE_VIEWER: &str = "Close viewer";

/// What opens what [`VIEWER`] shows in a tab of its own.
pub const OPEN_IN_NEW_TAB: &str = "Open in a new tab";

/// When a guest page's captcha can't load.
pub const CAPTCHA_FAILED: &str = "The captcha couldn't load.";

/// What signs in, as a link.
pub const SIGN_IN: &str = "Sign in";

/// After [`SIGN_IN`] on a guest's page.
pub const TO_CHAT_AS_YOURSELF: &str = "to chat as yourself.";

/// A guest's name field while it's empty.
pub const GUEST_NAME_HINT: &str = "How the team will see you";

/// A guest's name field's label: `Your name (optional)`.
#[must_use]
pub fn your_name_optional() -> String {
    format!("{} (optional)", super::YOUR_NAME)
}

/// While a guest joins.
pub const JOINING: &str = "Joining…";

/// What a guest joins with.
pub const START_CHATTING: &str = "Start chatting";

/// What a screen reader calls the composer's reply line.
pub const REPLYING: &str = "Replying";

/// What a screen reader calls the composer's completions.
pub const SUGGESTIONS: &str = "Suggestions";

/// The emoji picker, and the button that opens it.
pub const EMOJI: &str = "Emoji";

/// What a screen reader calls the search field.
pub const SEARCH_MESSAGES: &str = "Search messages";

/// What a screen reader calls a search's filters.
pub const FILTERS: &str = "Filters";

/// The list of who a guest's conversation can be assigned to; its button says it with `…`.
pub const ASSIGN_TO: &str = "Assign to";

/// [`ASSIGN_TO`] on the button that asks who, as a guest's header has it.
pub const ASSIGN_TO_LABEL: &str = "Assign to…";

/// The button that opens what else an admin can do to a guest.
pub const MORE_GUEST_ACTIONS: &str = "More guest actions";

/// What a screen reader calls the list of conversations.
pub const CONVERSATIONS: &str = "Conversations";

/// What a screen reader calls the sidebar's tabs.
pub const SIDEBAR: &str = "Sidebar";

/// What a screen reader calls the list of messages.
pub const MESSAGES: &str = "Messages";

/// The dot on a guest's conversation assigned to the person.
pub const ASSIGNED_TO_YOU: &str = "Assigned to you";

/// When a file's upload fails.
pub const UPLOAD_FAILED: &str = "The upload didn't go through.";

/// When zirc.dev's web app can't be reached.
pub const WEB_APP_UNREACHABLE: &str = "The web app couldn't be reached.";

/// A message's menu item that opens the emoji to react with: `React…`.
pub const REACT_PROMPT: &str = "React…";

/// What shows the organisations signed out of on this device, `count` of them: `Signed out
/// here (2)`.
#[must_use]
pub fn signed_out_here(count: usize) -> String {
    format!("Signed out here ({count})")
}

/// What closes something with Esc, said beside it.
pub const ESC_CLOSES: &str = "Esc closes";

/// Where something opens, on its button: `Open in #web`, `Open in Zed`.
#[must_use]
pub fn open_in_label(place: &str) -> String {
    format!("Open in {place}")
}

/// What signs in to `place`: `Sign in to Acme`, `Sign in to zirc.dev`.
#[must_use]
pub fn sign_in_to(place: &str) -> String {
    format!("{SIGN_IN} to {place}")
}

/// A sign-in's title: `Sign in to Zirc`.
#[must_use]
pub fn sign_in_to_zirc() -> String {
    sign_in_to(zirc_proto::NAME)
}

/// The button that signs in in the browser: `Sign in with zirc.dev`.
pub const SIGN_IN_WITH_ZIRC_DEV: &str = "Sign in with zirc.dev";

/// Under a sign-in's button: what signing in with zirc.dev does.
pub const SIGN_IN_NOTE: &str = "Sign in with your zirc.dev account, in your browser: it sets up your team's server and your name, and keeps your passphrase in the Keychain.";

/// Why files can't be shared while not signed in with zirc.dev.
pub const SIGN_IN_TO_SHARE: &str = "Sign in to zirc.dev to share files";

/// Why someone can't be reported while not signed in with zirc.dev.
pub const SIGN_IN_TO_REPORT: &str = "Sign in with zirc.dev to report.";

/// Why someone can't be blocked, or with `blocked` false unblocked, while not signed in
/// with zirc.dev.
#[must_use]
pub fn sign_in_to_block(blocked: bool) -> String {
    let what = if blocked { "block" } else { "unblock" };
    format!("Sign in with zirc.dev to {what} people.")
}

/// When the nick `wanted` was taken and the person is `me` on the server: what to do, with
/// `signed_in` whether they're signed in with zirc.dev.
#[must_use]
pub fn nick_taken(wanted: &str, me: &str, signed_in: bool) -> String {
    if signed_in {
        format!(
            "{wanted} is taken, so you're {me} here. Your account should hold it: check the \
             account in Settings."
        )
    } else {
        format!(
            "{wanted} is taken, so you're {me} here. Sign in with zirc.dev in Settings, and \
             your connections share {wanted} as your account."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_functions_say_what_goes_in_them() {
        assert_eq!(open_in_label("#web"), "Open in #web");
        assert_eq!(your_name_optional(), "Your name (optional)");
        assert_eq!(sign_in_to_zirc(), "Sign in to Zirc");
        assert_eq!(ASSIGN_TO_LABEL, format!("{ASSIGN_TO}…"));
        assert_eq!(
            sign_in_to_block(false),
            "Sign in with zirc.dev to unblock people."
        );
        assert!(nick_taken("ann", "ann_", false).starts_with("ann is taken, so you're ann_ here."));
    }
}
