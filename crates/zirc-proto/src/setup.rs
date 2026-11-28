//! zirc.dev's setup page, decided once: what its install-and-seat checklist, its say-hi step,
//! its tools step and the guided connect a job's tool comes back to say, from what zirc.dev and
//! the office's server know ([`Facts`]), as a [`Setup`]. The steps themselves, their order and
//! what proves each are [`crate::onboarding`]'s; what an office still has to connect is
//! [`crate::onboarding::missing`]'s. zirc.dev runs no Rust, so it reads [`setup`] through
//! zirc-media's wasm, and draws what it says.
//!
//! Metadata only: whether and when something happened, names zirc.dev holds, never what anyone
//! said.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ci::{CONNECT_ASK, CONNECT_COMMAND, TEST_FAILURE_COMMAND, test_failure_ask};
use crate::inbound::Tool;
use crate::onboarding::{SOME_COWORKER, words};

/// How CI stands for the office, from what zirc.dev and its server recorded: each when it first
/// happened, in unix seconds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "SetupCiFacts", optional_fields)]
pub struct CiFacts {
    /// A CI token was made for the office: `zirc ci connect` ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "number")]
    pub connected: Option<u64>,
    /// CI first sent zirc.dev something with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "number")]
    pub sent: Option<u64>,
    /// The office's server first heard CI in a channel ([`crate::onboarding::Signal::Ci`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "number")]
    pub heard: Option<u64>,
    /// CI last sent a test failure ([`TEST_FAILURE_COMMAND`], [`crate::ci::Data::test`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "number")]
    pub test: Option<u64>,
    /// The coworker was woken at its desk by CI, since that test failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "number")]
    pub woke: Option<u64>,
}

/// What zirc.dev and the office's server know of one person's setup, for [`setup`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "SetupFacts", optional_fields)]
pub struct Facts {
    /// The coworker setup seats, by the name it's shown by: `John`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coworker: Option<String>,
    /// The person's computer `zirc login` let in, by its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// A session on their computer started watching ([`crate::onboarding::Signal::Agent`]):
    /// zirc watches the repository it's in.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub watching: bool,
    /// A session of theirs online now, in one line ([`crate::working::line`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// A session of theirs sat at the coworker's desk ([`crate::onboarding::Signal::Seated`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub seated: bool,
    /// The repository the coworker works in, once a session sat at its desk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// The coworker woke on their direct message and answered
    /// ([`crate::onboarding::Signal::Answered`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub answered: bool,
    /// How CI stands.
    #[serde(default)]
    #[ts(as = "Option<CiFacts>", optional)]
    pub ci: CiFacts,
    /// The person's phone that signed in first, by its name: `Pixel 10`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
}

/// One part of a checklist that ticks itself off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "SetupItem", optional_fields)]
pub struct Item {
    /// Whether it happened.
    pub done: bool,
    /// What it is: `This computer is signed in`.
    pub text: String,
    /// What proves it, after a dot, where known: its computer's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// One thing that happened, as a step's timeline shows it as it comes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "SetupEvent")]
pub struct Event {
    /// When, in unix seconds.
    #[ts(type = "number")]
    pub at: u64,
    /// What happened: `Zirc heard CI`.
    pub text: String,
    /// Whether it's the coworker waking, which the page shows with its face and a link into
    /// the conversation.
    pub woke: bool,
}

/// A button that asks the coworker to do something: it puts [`Ask::message`] in the person's
/// conversation with it, for them to send as their own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "SetupAsk")]
pub struct Ask {
    /// The button: `Ask John to connect CI`.
    pub label: String,
    /// What it asks, one of every app's asks ([`CONNECT_ASK`], [`test_failure_ask`]).
    pub message: String,
    /// Beside the button, what the coworker then does.
    pub says: String,
}

/// What the setup page says, for one person now ([`setup`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "Setup", optional_fields)]
pub struct Setup {
    /// Install and seat's checklist: computer signed in, repository watched, a session joined,
    /// the coworker sat down.
    pub seat: Vec<Item>,
    /// Install and seat's first part, over the command: `In a terminal, in the repository John
    /// will work in`.
    pub install: String,
    /// Its second, over the sentence: `Then start a new Claude Code or Codex session for John,
    /// and tell it`.
    pub start: String,
    /// Under that: a session at the desk is the coworker's alone (the user's choice, 27 Nov 2026).
    pub alone: String,
    /// The sentence that seats a session: `You're John. Take your desk.`.
    pub sentence: String,
    /// Under the sentence, how the desk stands: `Waiting for John to sit down`.
    pub desk: Item,
    /// Say hi's line once the coworker answered: `John woke and answered you`.
    pub answered: Item,
    /// CI's timeline, oldest first.
    pub ci: Vec<Event>,
    /// The button CI's part shows now: connect, then a test failure; none once both were heard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci_ask: Option<Ask>,
    /// Whether CI's part is done: the server heard CI, as the tools step reads it. Its test
    /// failure is offered after, never waited for.
    pub ci_done: bool,
    /// Install and seat's first part folded to one line once all of it happened: `Installed,
    /// and watching web`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed: Option<String>,
    /// What say hi offers to write, each put in the composer for the person to send or change.
    pub suggestions: Vec<String>,
    /// Done's title: `John is at work`.
    pub at_work: String,
    /// Done's lines: at the desk, CI, the phone, each ticked once it happened.
    pub summary: Vec<Item>,
    /// Under them, what the person does from here (the user's choice, 28 Nov 2026: watch, and
    /// step in).
    pub watch: String,
}

/// Why CI comes first, under its name on the tools step.
pub const CI_ABOUT: &str = "So any coworker can wait for a build to land, and wake when one fails. Connected once, for the whole office.";

/// The tools step's part for the jobs' own tools when none is missing.
pub const JOBS_NEED_NOTHING: &str = "Nothing more: your coworkers' jobs need only CI.";

/// Below it: what happens when a later job needs a tool.
pub const JOBS_LATER: &str = "When a later job needs Sentry, PagerDuty or GitHub issues, Zirc walks you through connecting it then, and checks it works.";

/// An optional tool's line on the tools step: `Your coworkers wake on a new Linear issue, …`.
#[must_use]
pub fn optional_about(tool: Tool) -> String {
    format!(
        "Your coworkers wake on {}, with or without a job. Optional: skip it if you don't use it.",
        tool.wakes_on()
    )
}

/// The guided connect's head for a coworker whose job waits for a tool: `Get Grace working`.
#[must_use]
pub fn get_working(coworker: &str) -> String {
    format!("Get {coworker} working")
}

/// Under it.
pub const GET_WORKING_ABOUT: &str = "Only what your office doesn't have yet. Each part ticks off as it happens; leave any time and the card picks up where you stopped.";

/// The coworker's card's button while its job waits for a tool.
pub const FINISH_CONNECTING: &str = "Finish connecting";

/// The guided connect's last part, once the tool's test was heard: a test wakes nobody
/// ([`crate::inbound::HeardTool::last_test`]), so it says what will. `Heard Sentry's test:
/// Grace wakes on the next Sentry error.`
#[must_use]
pub fn heard_test(coworker: &str, tool: Tool) -> String {
    let next = tool
        .wakes_on()
        .trim_start_matches("a ")
        .trim_start_matches("an ");
    format!(
        "Heard {}'s test: {coworker} wakes on the next {next}.",
        tool.name()
    )
}

/// Before it's heard: `Waiting for Sentry's test`.
#[must_use]
pub fn waiting_for_test(tool: Tool) -> String {
    format!("Waiting for {}'s test", tool.name())
}

/// What the guided connect says for `coworker`, whose job waits for `tool` ([`guided`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "SetupGuided")]
pub struct Guided {
    /// Its head: `Get Grace working`.
    pub title: String,
    /// Under it.
    pub about: String,
    /// Its connect part: `Connect Sentry`.
    pub connect: String,
    /// Its check before the test is heard: `Waiting for Sentry's test`.
    pub waiting: String,
    /// Once it is: `Heard Sentry's test: Grace wakes on the next Sentry error.`
    pub heard: String,
}

/// The guided connect's words for `coworker` and `tool`.
#[must_use]
pub fn guided(coworker: &str, tool: Tool) -> Guided {
    Guided {
        title: get_working(coworker),
        about: GET_WORKING_ABOUT.to_owned(),
        connect: format!("Connect {}", tool.name()),
        waiting: waiting_for_test(tool),
        heard: heard_test(coworker, tool),
    }
}

/// What the setup page says for `facts`: one rule for every part of it.
#[must_use]
pub fn setup(facts: &Facts) -> Setup {
    let named = facts.coworker.as_deref();
    let coworker = named.unwrap_or(SOME_COWORKER);
    let seat = vec![
        Item {
            done: facts.machine.is_some(),
            text: "This computer is signed in".to_owned(),
            detail: facts.machine.clone(),
        },
        Item {
            done: facts.watching,
            text: "Zirc is watching your repository".to_owned(),
            detail: None,
        },
        Item {
            done: facts.watching || facts.session.is_some(),
            text: "A Claude Code or Codex session joined".to_owned(),
            detail: facts.session.clone(),
        },
        Item {
            done: facts.seated,
            text: format!("{} sat down", capital(coworker)),
            detail: facts.repository.as_ref().map(|repo| format!("in {repo}")),
        },
    ];
    let desk = Item {
        done: facts.seated,
        text: if facts.seated {
            format!("{} is at the desk", capital(coworker))
        } else {
            format!("Waiting for {coworker} to sit down")
        },
        detail: None,
    };
    let answered = Item {
        done: facts.answered,
        text: if facts.answered {
            format!("{} woke and answered you", capital(coworker))
        } else {
            format!("Waiting for {coworker} to answer")
        },
        detail: None,
    };
    let ci = &facts.ci;
    let mut events: Vec<Event> = [
        (
            ci.connected,
            format!("CI got its token: {CONNECT_COMMAND} ran"),
            false,
        ),
        (ci.sent, "CI sent Zirc a build's result".to_owned(), false),
        (ci.heard, "Zirc heard CI".to_owned(), false),
        (
            ci.test,
            "A test failure arrived: a build that failed on purpose".to_owned(),
            false,
        ),
        (ci.woke, format!("{} woke on it", capital(coworker)), true),
    ]
    .into_iter()
    .filter_map(|(at, text, woke)| at.map(|at| Event { at, text, woke }))
    .collect();
    events.sort_by_key(|event| event.at);
    let ci_ask = ci_ask(ci, coworker, named);
    let who = capital(coworker);
    let installed = installed(facts);
    let summary = summary(facts, coworker);
    Setup {
        install: format!("In a terminal, in the repository {coworker} will work in"),
        start: format!("Then start a new Claude Code or Codex session for {coworker}, and tell it"),
        alone: format!(
            "{who}'s session is {coworker}'s alone: what wakes {coworker} goes there, never into the session you work in. Keep your own tasks in other sessions; they still coordinate with {coworker}."
        ),
        seat,
        sentence: words(crate::onboarding::SENTENCE, named),
        desk,
        answered,
        ci_done: ci.heard.is_some(),
        ci: events,
        ci_ask,
        installed,
        suggestions: vec![
            format!("Hi {who}! What can you do?"),
            "What are you working on?".to_owned(),
            "What's changed in this repository lately?".to_owned(),
        ],
        at_work: format!("{who} is at work"),
        summary,
        watch: format!(
            "From here you mostly watch: your office shows what {coworker} is doing, and you can step in any time."
        ),
    }
}

/// The button CI's part shows now, for `coworker` (`named`, where it has a name).
fn ci_ask(ci: &CiFacts, coworker: &str, named: Option<&str>) -> Option<Ask> {
    if ci.connected.is_none() && ci.heard.is_none() {
        Some(Ask {
            label: format!("Ask {coworker} to connect CI"),
            message: CONNECT_ASK.to_owned(),
            says: format!(
                "{} runs {CONNECT_COMMAND}: it adds a step to your CI that tells Zirc, and opens a pull request for you to merge.",
                capital(coworker)
            ),
        })
    } else if ci.woke.is_none() {
        Some(Ask {
            label: format!("Ask {coworker} to fail a test build"),
            message: test_failure_ask(),
            says: format!(
                "{} runs {TEST_FAILURE_COMMAND}: a real build that fails on purpose, to see {} wake.",
                capital(coworker),
                named.unwrap_or("them")
            ),
        })
    } else {
        None
    }
}

/// Install and seat's first part as one line, once the computer is in and the repository watched.
fn installed(facts: &Facts) -> Option<String> {
    (facts.machine.is_some() && facts.watching).then(|| match facts.repository.as_deref() {
        Some(repo) => format!("Installed, and watching {repo}"),
        None => "Installed, and watching your repository".to_owned(),
    })
}

/// Done's lines for `coworker`: at the desk, CI, the phone.
fn summary(facts: &Facts, coworker: &str) -> Vec<Item> {
    let who = capital(coworker);
    let ci = &facts.ci;
    vec![
        Item {
            done: facts.seated,
            text: match (facts.seated, facts.repository.as_deref()) {
                (true, Some(repo)) => format!("At the desk in {repo}"),
                (true, None) => "At the desk".to_owned(),
                (false, _) => format!("Waiting for {coworker} to sit down"),
            },
            detail: facts.machine.clone(),
        },
        Item {
            done: ci.heard.is_some(),
            text: if ci.woke.is_some() {
                format!("Wakes when CI fails: {who} woke on the test build")
            } else if ci.heard.is_some() {
                "Wakes when CI fails".to_owned()
            } else {
                "CI isn't connected yet".to_owned()
            },
            detail: None,
        },
        Item {
            done: facts.phone.is_some(),
            text: match &facts.phone {
                Some(phone) => format!("On your {phone}, to watch and step in from anywhere"),
                None => "No phone yet: the app is at zirc.dev/app".to_owned(),
            },
            detail: None,
        },
    ]
}

/// `text` with its first letter a capital: "your coworker" as "Your coworker".
fn capital(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// These as TypeScript, for the web app's generated file.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        CiFacts::decl(config),
        Facts::decl(config),
        Item::decl(config),
        Event::decl(config),
        Ask::decl(config),
        Guided::decl(config),
        Setup::decl(config),
        format!("const SETUP_CI_ABOUT = {CI_ABOUT:?};"),
        format!("const SETUP_JOBS_NEED_NOTHING = {JOBS_NEED_NOTHING:?};"),
        format!("const SETUP_JOBS_LATER = {JOBS_LATER:?};"),
        format!("const SETUP_GET_WORKING_ABOUT = {GET_WORKING_ABOUT:?};"),
        format!("const SETUP_FINISH_CONNECTING = {FINISH_CONNECTING:?};"),
        format!(
            "const SETUP_OPTIONAL_ABOUT: Partial<Record<InboundTool, string>> = {{ {} }};",
            crate::onboarding::OPTIONAL_TOOLS
                .map(|tool| format!("{}: {:?}", tool.as_str(), optional_about(tool)))
                .join(", ")
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn john() -> Facts {
        Facts {
            coworker: Some("John".to_owned()),
            ..Facts::default()
        }
    }

    #[test]
    fn install_and_seat_ticks_off_each_part_as_it_happens() {
        let start = setup(&john());
        assert!(start.seat.iter().all(|item| !item.done));
        assert_eq!(start.seat[3].text, "John sat down");
        assert_eq!(start.desk.text, "Waiting for John to sit down");
        assert_eq!(start.sentence, "You're John. Take your desk.");
        assert_eq!(
            start.install,
            "In a terminal, in the repository John will work in"
        );
        assert!(start.alone.starts_with("John's session is John's alone"));
        let seated = setup(&Facts {
            machine: Some("henriks-mac".to_owned()),
            watching: true,
            seated: true,
            repository: Some("web".to_owned()),
            ..john()
        });
        assert!(seated.seat.iter().all(|item| item.done));
        assert_eq!(seated.seat[0].detail.as_deref(), Some("henriks-mac"));
        assert_eq!(seated.seat[3].detail.as_deref(), Some("in web"));
        assert_eq!(seated.desk.text, "John is at the desk");
    }

    #[test]
    fn say_hi_says_when_the_coworker_answered() {
        assert_eq!(setup(&john()).answered.text, "Waiting for John to answer");
        let answered = setup(&Facts {
            answered: true,
            ..john()
        });
        assert!(answered.answered.done);
        assert_eq!(answered.answered.text, "John woke and answered you");
    }

    #[test]
    fn ci_asks_to_connect_then_to_fail_a_test_build_then_nothing() {
        let ask = setup(&john()).ci_ask.unwrap();
        assert_eq!(ask.label, "Ask John to connect CI");
        assert_eq!(ask.message, CONNECT_ASK);
        let connected = setup(&Facts {
            ci: CiFacts {
                connected: Some(10),
                heard: Some(30),
                sent: Some(20),
                ..CiFacts::default()
            },
            ..john()
        });
        assert_eq!(connected.ci_ask.unwrap().message, test_failure_ask());
        assert!(connected.ci_done);
        assert!(
            !setup(&Facts {
                ci: CiFacts {
                    connected: Some(10),
                    ..CiFacts::default()
                },
                ..john()
            })
            .ci_done
        );
        let woke = setup(&Facts {
            ci: CiFacts {
                connected: Some(10),
                sent: Some(20),
                heard: Some(30),
                test: Some(40),
                woke: Some(41),
            },
            ..john()
        });
        assert!(woke.ci_ask.is_none() && woke.ci_done);
        let said: Vec<&str> = woke.ci.iter().map(|event| event.text.as_str()).collect();
        assert_eq!(
            said,
            [
                "CI got its token: zirc ci connect ran",
                "CI sent Zirc a build's result",
                "Zirc heard CI",
                "A test failure arrived: a build that failed on purpose",
                "John woke on it",
            ]
        );
        assert!(woke.ci.last().unwrap().woke);
    }

    #[test]
    fn the_guided_connect_says_what_a_tools_test_means() {
        assert_eq!(
            heard_test("Grace", Tool::Sentry),
            "Heard Sentry's test: Grace wakes on the next Sentry error."
        );
        assert_eq!(
            waiting_for_test(Tool::Pagerduty),
            "Waiting for PagerDuty's test"
        );
        assert_eq!(get_working("Grace"), "Get Grace working");
    }

    #[test]
    fn done_says_what_was_set_up_and_install_folds_once_it_happened() {
        let start = setup(&john());
        assert_eq!(start.installed, None);
        assert_eq!(start.at_work, "John is at work");
        assert!(start.summary.iter().all(|item| !item.done));
        assert_eq!(start.suggestions[0], "Hi John! What can you do?");
        let all = setup(&Facts {
            machine: Some("henriks-mac".to_owned()),
            watching: true,
            seated: true,
            repository: Some("web".to_owned()),
            phone: Some("Pixel 10".to_owned()),
            ci: CiFacts {
                heard: Some(30),
                woke: Some(41),
                ..CiFacts::default()
            },
            ..john()
        });
        assert_eq!(
            all.installed.as_deref(),
            Some("Installed, and watching web")
        );
        assert!(all.summary.iter().all(|item| item.done));
        let said: Vec<&str> = all.summary.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(
            said,
            [
                "At the desk in web",
                "Wakes when CI fails: John woke on the test build",
                "On your Pixel 10, to watch and step in from anywhere",
            ]
        );
        assert_eq!(all.summary[0].detail.as_deref(), Some("henriks-mac"));
    }

    #[test]
    fn without_a_coworker_it_says_your_coworker() {
        let page = setup(&Facts::default());
        assert_eq!(page.seat[3].text, "Your coworker sat down");
        assert_eq!(page.sentence, "You're <name>. Take your desk.");
    }
}
