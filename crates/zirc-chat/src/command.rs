//! What's typed on a message field: words to send, or a `/command`. Every zirc app reads
//! what's typed with [`parse`] and carries it out with zirc-chat's `Chat::run`, so a command
//! reads and does the same everywhere, and says the same back ([`said`]); what `Chat::run`
//! leaves to the app (the terminal client's picker, `/quote`, `/sessions`…) the app carries
//! out, or refuses with [`said::refused`].

use crate::chat::is_channel;

/// One line of input, understood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Words for the conversation shown. `//text` sends `/text`.
    Say(String),
    /// `/me waves`: an action in the conversation shown.
    Me(String),
    /// `/join #a,#b`: join channels, and show the first.
    Join(Vec<String>),
    /// `/browse`: list the channels we aren't in, to join, as zirc-chat's `Chat::browse`
    /// gives them.
    Browse,
    /// `/part [#channel] [reason]`: leave the channel shown, or the one named, once
    /// confirmed.
    Part {
        /// The channel, when named.
        channel: Option<String>,
        /// Why, if said.
        reason: Option<String>,
    },
    /// `/msg nick text`: a private message, or a message to another channel.
    Msg {
        /// The nick or channel.
        target: String,
        /// What to say.
        text: String,
    },
    /// `/query nick`: show the conversation with someone, without saying anything yet.
    /// `/query machine/session` is one with an agent session on our own machine.
    Query(String),
    /// `/agents`: list the agent sessions on our own machines we can talk to.
    Agents,
    /// `/queries [off|mine|anyone [yes]]`: say, or set, who may query this machine's
    /// agents, through zircd. `anyone` takes `yes`, after the warning.
    Queries(Option<String>),
    /// `/decide <about>: <words>`: record a decision of ours, from our own account, in
    /// `#ops` and the channel shown, for every session to find.
    Decide {
        /// What it covers.
        about: String,
        /// Our words.
        quote: String,
    },
    /// `/group nick…`: make a group direct message with these people.
    Group(Vec<String>),
    /// `/invite nick…`: let people into the group shown, as its maker.
    Invite(Vec<String>),
    /// `/close`: stop showing the conversation shown, leaving it if it's a channel; in a
    /// conversation with a guest, mark it resolved.
    Close,
    /// `/assign nick`: assign the conversation with a guest shown to someone on staff.
    Assign(String),
    /// `/ban`: refuse the guest shown in every public channel, for admins.
    Ban,
    /// `/unban [guest]`: let the guest named, or shown, back into the public channels, for
    /// admins.
    Unban(Option<String>),
    /// `/topic [text]`: show or set the topic of the channel shown.
    Topic(Option<String>),
    /// `/nick name`: change nick.
    Nick(String),
    /// `/away [message]`: say we're away, and why, or with nothing that we're back.
    Away(Option<String>),
    /// `/org [name]`: show the organisation named, or the next one, on a client signed in
    /// to several.
    Org(Option<String>),
    /// `/whois nick`: show who someone is and what they're doing.
    Whois(String),
    /// `/who handle`: show an agent session's card, by the name it goes by or its id.
    Who(String),
    /// `/react [emoji]`: react to the message picked with Shift-↑, or the newest, with the
    /// emoji named, or one picked from a picker.
    React(Option<String>),
    /// `/reply [text]`: answer the message picked with Shift-↑, or the newest, with `text`,
    /// or with nothing, start a reply to it that the next line sends.
    Reply(Option<String>),
    /// `/trim [reason]`: remove the message picked with Shift-↑, or the newest, from the
    /// server's history, once confirmed.
    Trim(Option<String>),
    /// `/quote LINE`: send a raw IRC line.
    Quote(String),
    /// `/typing [on|off]`: say, or set, whether others see when we're typing.
    Typing(Option<bool>),
    /// `/sessions`: show what every session is doing, as `zirc sessions` says.
    Sessions,
    /// `/profile`: open our account's page on zirc.dev, where our name and picture are set.
    Profile,
    /// `/people`: show only what people say, and whatever mentions us, or with that on,
    /// everything again.
    People,
    /// `/editor [choice]`: where code links open ("Open code in"), by a choice's id or
    /// label; with nothing, the choices, the one chosen marked.
    Editor(Option<String>),
    /// `/show [errors|warnings|info|everything]`: what the conversation shows, by what each
    /// line asks of its readers; with nothing, what it can show and what it shows now.
    Show(Option<String>),
    /// `/press <button>`: press a button, by its id or label, on the card picked or the
    /// latest card, once confirmed.
    Press(String),
    /// `/help`: list the commands.
    Help,
    /// `/quit [reason]`: leave.
    Quit(Option<String>),
}

impl Command {
    /// Its name, as typed after the `/` in full: `join` for `/j #api`, `say` and `me` for
    /// words.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Say(_) => "say",
            Self::Me(_) => "me",
            Self::Join(_) => "join",
            Self::Browse => "browse",
            Self::Part { .. } => "part",
            Self::Msg { .. } => "msg",
            Self::Query(_) => "query",
            Self::Agents => "agents",
            Self::Queries(_) => "queries",
            Self::Decide { .. } => "decide",
            Self::Group(_) => "group",
            Self::Invite(_) => "invite",
            Self::Close => "close",
            Self::Assign(_) => "assign",
            Self::Ban => "ban",
            Self::Unban(_) => "unban",
            Self::Topic(_) => "topic",
            Self::Nick(_) => "nick",
            Self::Away(_) => "away",
            Self::Org(_) => "org",
            Self::Whois(_) => "whois",
            Self::Who(_) => "who",
            Self::React(_) => "react",
            Self::Reply(_) => "reply",
            Self::Trim(_) => "trim",
            Self::Quote(_) => "quote",
            Self::Typing(_) => "typing",
            Self::Sessions => "sessions",
            Self::Profile => "profile",
            Self::People => "people",
            Self::Editor(_) => "editor",
            Self::Show(_) => "show",
            Self::Press(_) => "press",
            Self::Help => "help",
            Self::Quit(_) => "quit",
        }
    }

    /// The feature it uses, for analytics, if it's one that's counted.
    #[must_use]
    pub fn feature(&self) -> Option<zirc_proto::telemetry::Feature> {
        use zirc_proto::telemetry::Feature;
        let dm = |target: &str| {
            if target.contains('/') {
                Feature::AgentDm
            } else {
                Feature::Dm
            }
        };
        match self {
            Self::Join(_) => Some(Feature::TopicJoin),
            Self::Msg { target, .. } | Self::Query(target) if !is_channel(target) => {
                Some(dm(target))
            }
            Self::Group(_) => Some(Feature::Dm),
            Self::React(_) => Some(Feature::Reaction),
            _ => None,
        }
    }
}

/// The `command` that takes `on`, `off`, or nothing, as `/typing` does, with
/// `words`; `None` for anything else.
fn on_off(words: &str, command: fn(Option<bool>) -> Command) -> Option<Command> {
    match words.to_ascii_lowercase().as_str() {
        "" => Some(command(None)),
        "on" => Some(command(Some(true))),
        "off" => Some(command(Some(false))),
        _ => None,
    }
}

/// The command `name` that takes nothing after it, if it's one.
fn bare(name: &str) -> Option<Command> {
    Some(match name.to_ascii_lowercase().as_str() {
        "agents" => Command::Agents,
        "browse" => Command::Browse,
        "ban" => Command::Ban,
        "back" => Command::Away(None),
        "sessions" => Command::Sessions,
        "people" => Command::People,
        "help" | "h" => Command::Help,
        _ => return None,
    })
}

/// What a command says back when it can't do what was asked, the same in every app.
pub mod said {
    use super::Command;

    /// What's typed where nobody can be written to: the server's own conversation.
    pub const NOT_A_CONVERSATION: &str =
        "this isn't a conversation: /join a channel or /msg someone";
    /// What's typed in the Server channel that isn't an IRC line, such as nothing but tags.
    pub const NOT_A_LINE: &str =
        "the Server channel sends IRC lines as they are: PRIVMSG NickServ :HELP";
    /// What's typed where nobody reads it, when the conversation says nothing of its own.
    pub const TAKES_NO_MESSAGES: &str = "this conversation takes no messages";
    /// What's typed in a channel we aren't in.
    pub const JOIN_FIRST: &str = "join a channel first";
    /// `/invite` outside a group.
    pub const NOT_A_GROUP: &str = "/invite adds people to a group: start one with /group";
    /// `/invite` alone, with no guest shown.
    pub const INVITE_NEEDS: &str = "/invite needs a nick";
    /// `/invite` naming a guest among others.
    pub const GUEST_NOT_IN_GROUP: &str =
        "a guest can't join a group: /invite <guest> alone invites them to the organisation";
    /// `/part` alone outside a channel.
    pub const PART_NEEDS: &str = "/part from a channel, or name one";
    /// `/topic` alone in a channel with none.
    pub const NO_TOPIC: &str = "no topic";
    /// `/topic text` outside a channel.
    pub const TOPIC_FOR_CHANNELS: &str = "/topic is for channels";
    /// `/react` with nothing said yet.
    pub const NOTHING_TO_REACT: &str = "nothing here to react to yet";
    /// `/reply` with nothing said yet.
    pub const NOTHING_TO_REPLY: &str = "nothing here to reply to yet";
    /// `/close` in a guest's conversation that's resolved.
    pub const CLOSED_ALREADY: &str = "it's closed already";
    /// `/assign`, `/ban` or `/unban` outside a guest's conversation.
    pub const FOR_GUESTS: &str = "that's for a conversation with a guest";
    /// `/react` alone, in an app with no picker.
    pub const REACT_NEEDS: &str = "/react needs an emoji";
    /// `/reply` alone, in an app that doesn't start a reply from it.
    pub const REPLY_NEEDS: &str = "/reply needs what to say";

    /// `/react <typed>` when no emoji is called that.
    #[must_use]
    pub fn no_emoji(typed: &str) -> String {
        format!("no emoji called {typed}")
    }

    /// `/assign <to>` when they aren't on staff.
    #[must_use]
    pub fn not_staff(to: &str) -> String {
        format!("{to} isn't on staff here: assign it to someone who is")
    }

    /// A command the terminal client carries out and this app doesn't: `/quote is for
    /// zirc's terminal client`.
    #[must_use]
    pub fn elsewhere(name: &str) -> String {
        format!("/{name} is for zirc's terminal client")
    }

    /// What an app says of `command`, one `Chat::run` left to it and it doesn't carry out:
    /// what it needs, for `/react` and `/reply` alone, or that it's the terminal client's.
    #[must_use]
    pub fn refused(command: &Command) -> String {
        match command {
            Command::React(None) => REACT_NEEDS.to_owned(),
            Command::Reply(None) => REPLY_NEEDS.to_owned(),
            other => elsewhere(other.name()),
        }
    }
}

/// Why a `/command` couldn't be read, in words for the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unread {
    /// No command has this name.
    Unknown(String),
    /// The command needs more, and what: `/msg needs a nick and a message`.
    Needs(String),
}

impl std::fmt::Display for Unread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown(name) => write!(f, "no command /{name}: /help lists them"),
            Self::Needs(what) => f.write_str(what),
        }
    }
}

impl std::error::Error for Unread {}

impl From<&str> for Unread {
    fn from(what: &str) -> Self {
        Self::Needs(what.to_owned())
    }
}

/// Reads one line of input.
///
/// # Errors
///
/// When it's a command that's unknown or missing what it needs.
pub fn parse(input: &str) -> Result<Command, Unread> {
    let Some(command) = input.strip_prefix('/') else {
        return Ok(Command::Say(input.to_owned()));
    };
    if command.starts_with('/') {
        return Ok(Command::Say(command.to_owned()));
    }
    let (name, rest) = command.split_once(' ').unwrap_or((command, ""));
    let rest = rest.trim();
    let some = |text: &str| (!text.is_empty()).then(|| text.to_owned());
    let needs = |what: &str| Unread::Needs(format!("/{name} needs {what}"));
    let words = || rest.split_whitespace().map(str::to_owned).collect();
    let nick = || {
        rest.split_whitespace()
            .next()
            .map(str::to_owned)
            .ok_or_else(|| needs("a nick"))
    };
    if let Some(command) = bare(name) {
        return Ok(command);
    }
    match name.to_ascii_lowercase().as_str() {
        "me" => Ok(Command::Me(rest.to_owned())),
        "join" | "j" => {
            let channels: Vec<String> = rest
                .split([',', ' '])
                .filter(|c| !c.is_empty())
                .map(|c| {
                    if is_channel(c) {
                        c.to_owned()
                    } else {
                        format!("#{c}")
                    }
                })
                .collect();
            if channels.is_empty() {
                Err(needs("a channel"))
            } else {
                Ok(Command::Join(channels))
            }
        }
        "part" | "leave" => {
            let (channel, reason) = match rest.split_once(' ') {
                Some((first, reason)) if is_channel(first) => (some(first), some(reason.trim())),
                None if is_channel(rest) => (some(rest), None),
                _ => (None, some(rest)),
            };
            Ok(Command::Part { channel, reason })
        }
        "msg" | "m" => match rest.split_once(' ') {
            Some((target, text)) if !text.trim().is_empty() => Ok(Command::Msg {
                target: target.to_owned(),
                text: text.trim_start().to_owned(),
            }),
            _ => Err(needs("a nick and a message")),
        },
        "query" | "q" => nick().map(Command::Query),
        "queries" => Ok(Command::Queries(some(rest))),
        "decide" => match rest.split_once(':') {
            Some((about, quote)) if !about.trim().is_empty() && !quote.trim().is_empty() => {
                Ok(Command::Decide {
                    about: about.trim().to_owned(),
                    quote: quote.trim().to_owned(),
                })
            }
            _ => {
                Err("/decide needs what it's about and your words: /decide <about>: <words>".into())
            }
        },
        "group" => nick().map(|_| Command::Group(words())),
        "invite" => Ok(Command::Invite(words())),
        "close" | "wc" => Ok(Command::Close),
        "assign" => nick().map(Command::Assign),
        "unban" => Ok(Command::Unban(
            rest.split_whitespace().next().map(str::to_owned),
        )),
        "topic" => Ok(Command::Topic(some(rest))),
        "nick" => nick().map(Command::Nick),
        "org" => Ok(Command::Org(some(rest))),
        // words-guard: the command's name, not words for a person
        "away" => Ok(Command::Away(some(rest))),
        "whois" | "wi" => nick().map(Command::Whois),
        "who" => nick().map_err(|_| needs("a session")).map(Command::Who),
        "react" => Ok(Command::React(some(rest))),
        "trim" => Ok(Command::Trim(some(rest))),
        "typing" => on_off(rest, Command::Typing).ok_or_else(|| needs("on or off")),
        "reply" | "r" => Ok(Command::Reply(some(rest))),
        "quote" | "raw" => some(rest)
            .map(Command::Quote)
            .ok_or_else(|| needs("a line")),
        "profile" => Ok(Command::Profile),
        "show" => Ok(Command::Show(some(rest))),
        "editor" => Ok(Command::Editor(some(rest))),
        "press" => some(rest)
            .map(Command::Press)
            .ok_or_else(|| needs("a button")),
        "quit" | "exit" => Ok(Command::Quit(some(rest))),
        _ => Err(Unread::Unknown(name.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goes_away_and_comes_back() {
        assert_eq!(
            parse("/away in a meeting"),
            Ok(Command::Away(Some("in a meeting".into())))
        );
        assert_eq!(parse("/away"), Ok(Command::Away(None)));
        assert_eq!(parse("/back"), Ok(Command::Away(None)));
    }

    #[test]
    fn says_words_and_escaped_slashes() {
        assert_eq!(parse("hi there"), Ok(Command::Say("hi there".into())));
        assert_eq!(parse("//etc/hosts"), Ok(Command::Say("/etc/hosts".into())));
    }

    #[test]
    fn reads_commands() {
        assert_eq!(parse("/me waves"), Ok(Command::Me("waves".into())));
        assert_eq!(
            parse("/join api,#web"),
            Ok(Command::Join(vec!["#api".into(), "#web".into()]))
        );
        assert_eq!(
            parse("/part #api done for today"),
            Ok(Command::Part {
                channel: Some("#api".into()),
                reason: Some("done for today".into())
            })
        );
        assert_eq!(
            parse("/part bye"),
            Ok(Command::Part {
                channel: None,
                reason: Some("bye".into())
            })
        );
        assert_eq!(
            parse("/MSG bob  hello there"),
            Ok(Command::Msg {
                target: "bob".into(),
                text: "hello there".into()
            })
        );
        assert_eq!(parse("/quit"), Ok(Command::Quit(None)));
        assert_eq!(parse("/agents"), Ok(Command::Agents));
        assert_eq!(parse("/sessions"), Ok(Command::Sessions));
        assert_eq!(
            parse("/decide cloud spend: yes, add the node"),
            Ok(Command::Decide {
                about: "cloud spend".into(),
                quote: "yes, add the node".into()
            })
        );
        assert!(parse("/decide no colon").is_err());
        assert_eq!(parse("/queries"), Ok(Command::Queries(None)));
        assert_eq!(
            parse("/queries anyone yes"),
            Ok(Command::Queries(Some("anyone yes".into())))
        );
        assert_eq!(
            parse("/group bob carol"),
            Ok(Command::Group(vec!["bob".into(), "carol".into()]))
        );
        assert_eq!(
            parse("/invite dave"),
            Ok(Command::Invite(vec!["dave".into()]))
        );
        assert_eq!(parse("/group"), Err("/group needs a nick".into()));
        assert_eq!(
            parse("/react tada"),
            Ok(Command::React(Some("tada".into())))
        );
        assert_eq!(parse("/react"), Ok(Command::React(None)));
        assert_eq!(
            parse("/reply  on it "),
            Ok(Command::Reply(Some("on it".into())))
        );
        assert_eq!(parse("/r"), Ok(Command::Reply(None)));
        assert_eq!(parse("/assign dana"), Ok(Command::Assign("dana".into())));
        assert_eq!(parse("/assign"), Err("/assign needs a nick".into()));
        assert_eq!(parse("/ban"), Ok(Command::Ban));
        assert_eq!(parse("/profile"), Ok(Command::Profile));
        assert_eq!(parse("/unban"), Ok(Command::Unban(None)));
        assert_eq!(parse("/invite"), Ok(Command::Invite(Vec::new())));
        assert_eq!(parse("/group"), Err("/group needs a nick".into()));
    }

    #[test]
    fn explains_what_is_missing() {
        assert_eq!(
            parse("/msg bob"),
            Err("/msg needs a nick and a message".into())
        );
        assert_eq!(parse("/join"), Err("/join needs a channel".into()));
        assert_eq!(
            parse("/frobnicate"),
            Err(Unread::Unknown("frobnicate".into()))
        );
        assert!(
            parse("/frobnicate")
                .unwrap_err()
                .to_string()
                .contains("/help")
        );
    }

    #[test]
    fn counts_the_features_analytics_counts() {
        use zirc_proto::telemetry::Feature;
        let feature = |input| parse(input).unwrap().feature();
        assert_eq!(feature("/msg bob hi"), Some(Feature::Dm));
        assert_eq!(feature("/query mattias-mbp/cc-1"), Some(Feature::AgentDm));
        assert_eq!(feature("/group bob carol"), Some(Feature::Dm));
        assert_eq!(feature("/react tada"), Some(Feature::Reaction));
        assert_eq!(feature("/join #zirc"), Some(Feature::TopicJoin));
        assert_eq!(feature("/msg #zirc hi"), None);
        assert_eq!(feature("hello"), None);
    }
}
