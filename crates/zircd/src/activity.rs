//! What zircd says it does, one plain line each, on its standard output: a session joining
//! or leaving, an overlap told, a push announced, a message delivered, a wake, a request
//! asked or withdrawn, the connection coming or going. Run in a terminal tab, as people run
//! it until they put it in the background (`zirc daemon install`), that's all the tab
//! shows: its tracing log goes to `zircd.log` ([`crate::log`]). Under launchd or systemd the
//! lines go where its log does.
//!
//! Each says what happened and to whom, with the time, and never what anyone wrote: who's
//! told of what is the person's to watch, what was said stays in their apps.

use zirc_core::ipc::Why;
use zirc_proto::request::Withdrawal;

/// Where a daemon's lines go: standard output, and in tests a list too.
#[derive(Debug, Default)]
pub(crate) struct Activity {
    /// The organisation's name, said before each line on a machine signed in to several.
    org: Option<String>,
    /// Each line said, without its time, for tests to read.
    #[cfg(test)]
    pub(crate) said: Vec<String>,
}

impl Activity {
    /// The lines of the daemon for organisation `org`, named in each only when the machine
    /// is signed in to `several`.
    pub(crate) fn new(org: &str, several: bool) -> Self {
        Self {
            org: several.then(|| org.to_owned()),
            #[cfg(test)]
            said: Vec::new(),
        }
    }

    /// Says `what`, at the local time.
    pub(crate) fn say(&mut self, what: &str) {
        let at = chrono::Local::now().format("%H:%M");
        match &self.org {
            Some(org) => println!("{at} {org}: {what}"),
            None => println!("{at} {what}"),
        }
        #[cfg(test)]
        self.said.push(what.to_owned());
    }
}

/// A session as a line names it: by its name, with its id, or by its id alone.
pub(crate) fn who(session: &str, name: Option<&str>) -> String {
    match name {
        Some(name) if name != session => format!("{name} ({session})"),
        _ => session.to_owned(),
    }
}

pub(crate) fn joined(who: &str) -> String {
    format!("{who} joined")
}

pub(crate) fn left(who: &str) -> String {
    format!("{who} left")
}

pub(crate) fn connected(server: &str, nick: &str) -> String {
    format!("connected to {server} as {nick}")
}

pub(crate) fn disconnected(server: &str) -> String {
    format!("lost the connection to {server}: connecting again")
}

/// `paths` as a line says them: the first, and how many more.
fn files(paths: &[String]) -> String {
    match paths {
        [] => "their files".to_owned(),
        [one] => one.clone(),
        [first, rest @ ..] => format!("{first} and {} more", rest.len()),
    }
}

/// An overlap told to `told`, the sessions here it woke, about `with`, whom it's with when
/// that isn't one of them, in `paths`.
pub(crate) fn overlap(told: &[String], with: Option<&str>, paths: &[String]) -> String {
    let told = match told {
        [one, two] => format!("{one} and {two}"),
        _ => told.join(", "),
    };
    let with = with.map_or_else(String::new, |with| format!(" with {with}"));
    format!("told {told} of an overlap{with} in {}", files(paths))
}

pub(crate) fn pushed(who: &str, branch: &str, channel: &str) -> String {
    format!("announced {who}'s push of {branch} in {channel}")
}

/// A message delivered to `to`: `what` it is, such as `a dm`, and whom it's `from`.
pub(crate) fn delivered(what: &str, from: &str, to: &str) -> String {
    format!("delivered {what} from {from} to {to}")
}

/// A wake of `who`'s watch, for `why`, by what `from` said in `channel`, when said there.
pub(crate) fn woke(who: &str, why: Why, from: Option<&str>, channel: Option<&str>) -> String {
    let by = from.map_or_else(String::new, |from| format!(" from {from}"));
    let channel = channel.map_or_else(String::new, |channel| format!(" in {channel}"));
    format!("woke {who}: {}{by}{channel}", reason(why))
}

/// What woke a watch, as a line says it.
fn reason(why: Why) -> &'static str {
    match why {
        Why::Overlap => "an overlap",
        Why::Direct => "a message",
        Why::Mention => "a mention",
        Why::Paths => "a post about its files",
        Why::Intent => "an intent on its files",
        Why::Topic => "a post in a topic it joined",
        Why::Landing => "a landing that changes its files",
        Why::StackedOn => "a push built on its branch",
        Why::Channel => "a post in a channel it watches",
        Why::Decision => "a decision",
        Why::Reacted => "a reaction to its message",
        Why::Awaited => "CI on what it waits for",
        Why::BranchLanded => "a landing it was told of",
        Why::StateChanged => "a state it waits on",
        Why::ReactionAwaited => "the reaction it waits for",
        Why::DefaultMoved => "landings on the default branch",
        Why::Other => "news",
    }
}

pub(crate) fn asked(who: &str, by: &str, id: &str) -> String {
    format!("{who} asked {by} a yes or no ({id})")
}

pub(crate) fn withdrawn(who: &str, id: &str, why: Withdrawal) -> String {
    match why {
        Withdrawal::Asked => format!("{who} withdrew its request {id}"),
        other => format!("withdrew {who}'s request {id}: it {}", other.said()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|&path| path.to_owned()).collect()
    }

    #[test]
    fn says_what_happened_and_to_whom() {
        let ada = who("cc-1a2b3c", Some("login"));
        assert_eq!(ada, "login (cc-1a2b3c)");
        assert_eq!(who("cc-1", Some("cc-1")), "cc-1");
        assert_eq!(who("cc-1", None), "cc-1");
        assert_eq!(joined(&ada), "login (cc-1a2b3c) joined");
    }

    #[test]
    fn says_what_was_told_and_to_whom_never_what() {
        assert_eq!(
            overlap(
                &["cc-1".into(), "cc-2".into()],
                None,
                &paths(&["a.rs", "b.rs"])
            ),
            "told cc-1 and cc-2 of an overlap in a.rs and 1 more"
        );
        assert_eq!(
            overlap(&["cc-1".into()], Some("raj-mbp"), &paths(&["a.rs"])),
            "told cc-1 of an overlap with raj-mbp in a.rs"
        );
        assert_eq!(
            pushed("cc-1", "login-flow", "#agents"),
            "announced cc-1's push of login-flow in #agents"
        );
        assert_eq!(
            delivered("a dm", "mattias", "cc-1"),
            "delivered a dm from mattias to cc-1"
        );
        assert_eq!(
            woke("cc-1", Why::Mention, Some("raj"), Some("#agents")),
            "woke cc-1: a mention from raj in #agents"
        );
    }

    #[test]
    fn says_a_request_asked_and_withdrawn() {
        assert_eq!(
            asked("cc-1", "mattias", "mbp/3"),
            "cc-1 asked mattias a yes or no (mbp/3)"
        );
        assert_eq!(
            withdrawn("cc-1", "mbp/3", Withdrawal::Asked),
            "cc-1 withdrew its request mbp/3"
        );
        assert_eq!(
            withdrawn("cc-1", "mbp/3", Withdrawal::Done),
            "withdrew cc-1's request mbp/3: it finished its task"
        );
    }

    #[test]
    fn names_the_organisation_only_on_a_machine_in_several() {
        let mut one = Activity::new("zirc", false);
        one.say("cc-1 joined");
        assert_eq!(one.org, None);
        assert_eq!(one.said, ["cc-1 joined"]);
        assert_eq!(Activity::new("zirc", true).org.as_deref(), Some("zirc"));
    }
}
