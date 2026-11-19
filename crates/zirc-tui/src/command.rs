//! What's typed on the input line, read as every zirc app reads it ([`zirc_core::command`]),
//! and the help that lists it.

pub use zirc_core::command::{Command, parse};

/// The commands, as `/help` lists them.
pub const HELP: &[&str] = &[
    "/join #channel[,#other]   join channels and show the first",
    "/part [#channel] [reason] leave the channel shown, or the one named, once you've",
    "                          said y",
    "/browse                   the channels you're not in, to /join: Tab completes",
    "/msg nick text            send a private message",
    "/query nick               open a conversation with someone",
    "/agents                   list the agents on your machines, each to /query",
    "/queries [off|mine|anyone] who may query this machine's agents (zirc ask)",
    "/decide <about>: <words>  record a decision of yours for every session to find",
    "/group nick [nick…]       start a group direct message with them",
    "/invite nick [nick…]      add people to the group shown, if you started it",
    "/me text                  say what you're doing",
    "/topic [text]             show or set the topic",
    "/nick name                change your nickname",
    "/away [message]           say you're away, or with nothing that you're back",
    "/whois nick               who someone is and what they're doing",
    "/who session              an agent session's card: what it's doing and changing",
    "/react [emoji]            react to the message picked, or the newest: 🎉, tada or",
    "                          :+1:, or with nothing, pick one",
    "/reply [text]             answer the message picked, or the newest; with nothing,",
    "                          the next line you send answers it",
    "/trim [reason]            remove the message picked, or the newest, from the",
    "                          server's history, such as a secret pasted by mistake",
    "/close                    close the conversation shown; with a guest, resolve it",
    "/typing [on|off]          whether others see when you're typing",
    "/people                   only what people say, and what mentions you; again for all",
    "/show [errors|warnings|info|everything]  what this conversation shows; what names",
    "                          you always shows",
    "/press <button>           press a card's button, [Retry], once you've said y",
    "/sessions                 what every session is doing: Alt-S too, Esc closes it",
    "/editor [choice]          where code links open: automatic, an editor or forge",
    "/assign nick              assign the conversation with a guest to someone",
    "/ban                      refuse the guest shown in every public channel (admins)",
    "/unban [guest]            let a guest back into the public channels (admins)",
    "/invite [guest]           invite a guest to join the organisation (admins)",
    "/profile                  set your name and picture, on zirc.dev",
    "/quote LINE               send a raw IRC line",
    "/quit [reason]            leave",
    "Keys: Ctrl-N/Ctrl-P or Alt-↓/Alt-↑ switch, Alt-1…9 jump, Alt-A next unread,",
    "      Alt-P pins the conversation shown to the top of its section, or unpins it,",
    "      PgUp/PgDn scroll, Tab completes nicks, Ctrl-C quits. Shift-Enter (or",
    "      Alt-Enter) starts a new line. Esc marks the conversation shown read,",
    "      Shift-Esc every one.",
    "      Shift-↑/Shift-↓ pick a message; then ↑/↓ pick another, r replies to it, y",
    "      copies it, l copies its link, Alt-R reacts to it and Esc lets it go.",
    "Mouse: click a conversation to show it, someone in the member list to see who",
    "       they are (Enter talks to them), a message to pick it, or a reaction to give",
    "       or take back your own; the wheel scrolls. Hold Shift (Option in iTerm2,",
    "       Fn in Terminal) to select text, or start with --no-mouse.",
];

/// The help for switching organisations, shown only when signed in to several: someone in
/// one organisation sees no change.
pub const ORG_HELP: &[&str] = &[
    "/org [name]               show the organisation named, or the next one",
    "Keys: Alt-O shows the next organisation.",
];
