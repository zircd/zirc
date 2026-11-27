//! zirc.dev's home page for someone signed in, decided once (the user's choices, 27 Nov 2026,
//! the `HomeSignedIn` mock): with an office, the hero is that office — a welcome, how many
//! coworkers are at their desks, their badges, the hot desks in it now and a way to hire
//! another; with its setup unfinished, where they left off; signed in with no office, the
//! coworker their browser kept, still waiting for a desk. Signed out, the page is as it was.
//!
//! What waits for the person's answer is the chat's ([`waiting`] says how much, for the line
//! under the welcome), as only an app's own connection reads it. zirc.dev runs no Rust, so it
//! reads [`home`] through zirc-media's wasm and draws what it says; the words it fills in
//! itself in a browser ([`KEPT_TITLE`] and the rest) are templates with [`NAME`] in them.
//!
//! Metadata only: names zirc.dev holds and whether desks are taken, never what anyone said.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::onboarding::{self, id, words};

/// One of the office's coworkers, as the home page counts them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "HomeCoworkerFact", optional_fields)]
pub struct CoworkerFact {
    /// The name it's shown by: `John`.
    pub name: String,
    /// Whether a session sits at its desk now.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub seated: bool,
}

/// A session at a hot desk in the office now: online, at no coworker's desk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "HomeHotDeskFact", optional_fields)]
pub struct HotDeskFact {
    /// Whose it is, by the name they're shown by: `Raj`. None for the person's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whose: Option<String>,
    /// The name it goes by: `checkout-form`, or its id.
    pub name: String,
}

/// Where the person's setup of the office stands, while it's unfinished.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "HomeSetupFact", optional_fields)]
pub struct SetupFact {
    /// The next step's number ([`onboarding::Progress::next`]).
    pub next: u8,
    /// The coworker setup seats, by the name it's shown by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coworker: Option<String>,
}

/// What zirc.dev knows of a signed-in person and the office the home page shows, for [`home`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "HomeFacts", optional_fields)]
pub struct Facts {
    /// The person's name, where their profile gives one: `Henrik Lind`. The welcome says the
    /// first of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    /// The office's name: `Henrik's office`.
    pub office: String,
    /// Its coworkers, in the order they were hired.
    #[serde(default)]
    pub coworkers: Vec<CoworkerFact>,
    /// Whether the office's server answered for its desks: without it, nobody is counted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub unreachable: bool,
    /// The sessions at hot desks in it now.
    #[serde(default)]
    pub hot_desks: Vec<HotDeskFact>,
    /// Where the person's setup stands; none once it's done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<SetupFact>,
}

/// A run of words in a line, set in mono where it's a name the person typed or a session goes
/// by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "HomePart", optional_fields)]
pub struct Part {
    /// The words.
    pub text: String,
    /// Whether they're set as code.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub code: bool,
}

/// The hero while the person's setup is unfinished.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "HomeSetup")]
pub struct Setup {
    /// Over the headline: `Setting up Henrik's office · step 4 of 7`.
    pub kicker: String,
    /// The headline: `Pick up where you left off: say hi to John`.
    pub title: String,
    /// Under it, what the step is for, as setup says it.
    pub about: String,
    /// The button into setup: [`CARRY_ON`].
    pub button: String,
}

/// What the home page's hero says for one person and office now ([`home`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "Home", optional_fields)]
pub struct Home {
    /// The headline: `Welcome back, Henrik`.
    pub welcome: String,
    /// Under it, how the desks stand: `Three of four at their desks.` None with no coworkers
    /// or a server that didn't answer. What waits for the person follows it, from the chat
    /// ([`waiting`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desks: Option<String>,
    /// In place of the badges while there are none: [`NO_COWORKERS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_coworkers: Option<String>,
    /// Under the badges, the hot desks in the office now: `Also in the office now: 2 hot desks,
    /// Henrik's `checkout-form` and Raj's `search-index`.` Empty while there are none.
    pub hot_desks: Vec<Part>,
    /// The link to the hire page: [`HIRE_ANOTHER`], or [`HIRE_FIRST`] with no coworkers.
    pub hire: String,
    /// The hero while setup is unfinished, in place of the rest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<Setup>,
}

/// The welcome without a name to say.
pub const WELCOME: &str = "Welcome back";

/// The link to the hire page under the badges.
pub const HIRE_ANOTHER: &str = "Hire another coworker";

/// The same with no coworkers yet, as the apps' Office says it under [`NO_COWORKERS`].
pub const HIRE_FIRST: &str = "Hire a coworker";

/// In place of the badges with no coworkers yet, as the apps' Office says it.
pub const NO_COWORKERS: &str = "No coworkers yet.";

/// A coworker's badge strip while one of its questions waits for the person, as the apps'
/// Office says it.
pub const ASKED_YOU: &str = "Asked you";

/// The setup hero's button.
pub const CARRY_ON: &str = "Carry on setting up";

/// What heads the switcher's menu of the person's other offices.
pub const SWITCH_OFFICE: &str = "Switch office";

/// The header's button, signed in, into the person's office.
pub const YOUR_OFFICE: &str = "Your office";

/// What the templates below write for a coworker's or a person's name.
pub const NAME: &str = "{name}";

/// Signed in with no office, over the headline: `Signed in as Henrik`.
pub const SIGNED_IN_AS: &str = "Signed in as {name}";

/// Signed in with no office and a coworker kept in the browser: `John is still waiting for a
/// desk`.
pub const KEPT_TITLE: &str = "{name} is still waiting for a desk";

/// Under it.
pub const KEPT_ABOUT: &str =
    "You hired {name} in this browser. Open your office and {name} comes along.";

/// The kept coworker's badge strip.
pub const KEPT_STRIP: &str = "Hired, no office yet";

/// Signed in with no office and nobody kept: the headline.
pub const NO_OFFICE_TITLE: &str = "Open your office";

/// Under it.
pub const NO_OFFICE_ABOUT: &str =
    "Hire your first coworker, and the office it works in opens with it.";

/// Signed in with no office, the button: on to making it, with the kept coworker if there's one.
pub const FINISH_OPENING: &str = "Finish opening your office";

/// The same with nobody kept: on to hiring.
pub const HIRE_YOUR_FIRST: &str = "Hire your first coworker";

/// How many coworkers or questions, as a sentence starts with it: `Three`, `12`.
fn counted(n: usize) -> String {
    const WORDS: [&str; 13] = [
        "No", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine", "Ten",
        "Eleven", "Twelve",
    ];
    WORDS
        .get(n)
        .map_or_else(|| n.to_string(), |word| (*word).to_owned())
}

/// The same inside a sentence: `four`.
fn counted_lower(n: usize) -> String {
    let word = counted(n);
    if n < 13 { word.to_lowercase() } else { word }
}

/// Whose a hot desk's session is, in a sentence: `your`, `Raj's`.
fn whose(owner: Option<&str>) -> String {
    owner.map_or_else(|| "your".to_owned(), crate::possessive)
}

/// How the desks stand: `Three of four at their desks.`, `All five at their desks.`, `John
/// is at the desk.`, `Nobody at a desk right now.`
fn desks(coworkers: &[CoworkerFact]) -> Option<String> {
    let seated = coworkers.iter().filter(|c| c.seated).count();
    Some(match (coworkers, seated) {
        ([], _) => return None,
        ([one], 1) => format!("{} is at the desk.", one.name),
        ([one], _) => format!("{}'s desk is empty.", one.name),
        (_, 0) => "Nobody at a desk right now.".to_owned(),
        ([_, _], 2) => "Both at their desks.".to_owned(),
        (all, n) if n == all.len() => format!("All {} at their desks.", counted_lower(n)),
        (all, n) => format!(
            "{} of {} at their desks.",
            counted(n),
            counted_lower(all.len())
        ),
    })
}

/// How many hot desks it names before saying how many more.
const HOT_DESKS_NAMED: usize = 3;

/// The hot desks in a line, their names in mono.
fn hot_desks(desks: &[HotDeskFact]) -> Vec<Part> {
    let words = |text: String| Part { text, code: false };
    let code = |text: &str| Part {
        text: text.to_owned(),
        code: true,
    };
    let Some(first) = desks.first() else {
        return Vec::new();
    };
    let lead = if desks.len() == 1 {
        "Also in the office now: a hot desk, ".to_owned()
    } else {
        format!("Also in the office now: {} hot desks, ", desks.len())
    };
    let mut parts = vec![
        words(format!("{lead}{} ", whose(first.whose.as_deref()))),
        code(&first.name),
    ];
    let named = desks.len().min(HOT_DESKS_NAMED);
    let more = desks.len() - named;
    for (at, desk) in desks.iter().enumerate().take(named).skip(1) {
        let joint = if at == named - 1 && more == 0 {
            " and "
        } else {
            ", "
        };
        parts.push(words(format!("{joint}{} ", whose(desk.whose.as_deref()))));
        parts.push(code(&desk.name));
    }
    parts.push(words(if more > 0 {
        format!(" and {more} more.")
    } else {
        ".".to_owned()
    }));
    parts
}

/// The setup hero for the person's next step.
fn setup(office: &str, fact: &SetupFact) -> Option<Setup> {
    let total = onboarding::STEPS.len();
    let step = onboarding::step(fact.next).filter(|step| step.id != id::DONE)?;
    let coworker = fact.coworker.as_deref();
    let title = words(step.title, coworker);
    let mut lower = title.chars();
    let title = lower
        .next()
        .map(|c| c.to_lowercase().chain(lower).collect::<String>())
        .unwrap_or_default();
    Some(Setup {
        kicker: format!("Setting up {office} · step {} of {total}", step.number),
        title: format!("Pick up where you left off: {title}"),
        about: words(step.about, coworker),
        button: CARRY_ON.to_owned(),
    })
}

/// What the home page's hero says for `facts`: the one place it's decided.
#[must_use]
pub fn home(facts: &Facts) -> Home {
    let welcome = facts
        .person
        .as_deref()
        .and_then(|name| name.split_whitespace().next())
        .map_or_else(|| WELCOME.to_owned(), |first| format!("{WELCOME}, {first}"));
    let none = facts.coworkers.is_empty();
    Home {
        welcome,
        desks: if facts.unreachable {
            None
        } else {
            desks(&facts.coworkers)
        },
        no_coworkers: none.then(|| NO_COWORKERS.to_owned()),
        hot_desks: hot_desks(&facts.hot_desks),
        hire: if none { HIRE_FIRST } else { HIRE_ANOTHER }.to_owned(),
        setup: facts
            .setup
            .as_ref()
            .and_then(|fact| setup(&facts.office, fact)),
    }
}

/// What waits for the person's answer, after how the desks stand: `One question is waiting for
/// you.`, `Two questions are waiting for you.`; none while nothing does. The chat counts them,
/// as Waiting for you shows them.
#[must_use]
pub fn waiting(n: usize) -> Option<String> {
    match n {
        0 => None,
        1 => Some("One question is waiting for you.".to_owned()),
        n => Some(format!("{} questions are waiting for you.", counted(n))),
    }
}

/// The TypeScript of what zirc.dev reads.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        CoworkerFact::decl(config),
        HotDeskFact::decl(config),
        SetupFact::decl(config),
        Facts::decl(config),
        Part::decl(config),
        Setup::decl(config),
        Home::decl(config),
        format!("const HOME_ASKED_YOU = {ASKED_YOU:?};"),
        format!("const HOME_SWITCH_OFFICE = {SWITCH_OFFICE:?};"),
        format!("const HOME_YOUR_OFFICE = {YOUR_OFFICE:?};"),
        format!("const HOME_NAME = {NAME:?};"),
        format!("const HOME_SIGNED_IN_AS = {SIGNED_IN_AS:?};"),
        format!("const HOME_KEPT_TITLE = {KEPT_TITLE:?};"),
        format!("const HOME_KEPT_ABOUT = {KEPT_ABOUT:?};"),
        format!("const HOME_KEPT_STRIP = {KEPT_STRIP:?};"),
        format!("const HOME_NO_OFFICE_TITLE = {NO_OFFICE_TITLE:?};"),
        format!("const HOME_NO_OFFICE_ABOUT = {NO_OFFICE_ABOUT:?};"),
        format!("const HOME_FINISH_OPENING = {FINISH_OPENING:?};"),
        format!("const HOME_HIRE_YOUR_FIRST = {HIRE_YOUR_FIRST:?};"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coworker(name: &str, seated: bool) -> CoworkerFact {
        CoworkerFact {
            name: name.to_owned(),
            seated,
        }
    }

    fn desk(whose: Option<&str>, name: &str) -> HotDeskFact {
        HotDeskFact {
            whose: whose.map(str::to_owned),
            name: name.to_owned(),
        }
    }

    fn office(coworkers: Vec<CoworkerFact>) -> Facts {
        Facts {
            person: Some("Henrik".to_owned()),
            office: "Henrik's office".to_owned(),
            coworkers,
            ..Facts::default()
        }
    }

    fn line(parts: &[Part]) -> String {
        parts
            .iter()
            .map(|p| {
                if p.code {
                    format!("`{}`", p.text)
                } else {
                    p.text.clone()
                }
            })
            .collect()
    }

    #[test]
    fn the_office_says_who_is_at_their_desk() {
        let four = office(vec![
            coworker("John", true),
            coworker("Grace", true),
            coworker("Sam", true),
            coworker("Noor", false),
        ]);
        let home = home(&four);
        assert_eq!(home.welcome, "Welcome back, Henrik");
        assert_eq!(home.desks.as_deref(), Some("Three of four at their desks."));
        assert_eq!(home.hire, HIRE_ANOTHER);
        assert_eq!(home.no_coworkers, None);
        assert_eq!(home.setup, None);
    }

    #[test]
    fn every_count_of_desks_reads_as_a_sentence() {
        let said = |c: Vec<CoworkerFact>| desks(&c);
        assert_eq!(said(vec![]), None);
        assert_eq!(
            said(vec![coworker("John", true)]).unwrap(),
            "John is at the desk."
        );
        assert_eq!(
            said(vec![coworker("John", false)]).unwrap(),
            "John's desk is empty."
        );
        assert_eq!(
            said(vec![coworker("A", true), coworker("B", true)]).unwrap(),
            "Both at their desks."
        );
        assert_eq!(
            said(vec![coworker("A", false), coworker("B", false)]).unwrap(),
            "Nobody at a desk right now."
        );
        assert_eq!(
            said(vec![
                coworker("A", true),
                coworker("B", true),
                coworker("C", true)
            ])
            .unwrap(),
            "All three at their desks."
        );
        let many: Vec<_> = (0..14).map(|n| coworker(&n.to_string(), n < 13)).collect();
        assert_eq!(said(many).unwrap(), "13 of 14 at their desks.");
    }

    #[test]
    fn a_server_that_does_not_answer_counts_nobody() {
        let mut facts = office(vec![coworker("John", false)]);
        facts.unreachable = true;
        assert_eq!(home(&facts).desks, None);
    }

    #[test]
    fn no_coworkers_offers_the_first_hire() {
        let home = home(&office(vec![]));
        assert_eq!(home.desks, None);
        assert_eq!(home.no_coworkers.as_deref(), Some("No coworkers yet."));
        assert_eq!(home.hire, "Hire a coworker");
    }

    #[test]
    fn hot_desks_are_named_whose_they_are() {
        assert!(hot_desks(&[]).is_empty());
        assert_eq!(
            line(&hot_desks(&[
                desk(Some("Henrik"), "checkout-form"),
                desk(Some("Raj"), "search-index")
            ])),
            "Also in the office now: 2 hot desks, Henrik's `checkout-form` and Raj's `search-index`."
        );
        assert_eq!(
            line(&hot_desks(&[desk(None, "tls")])),
            "Also in the office now: a hot desk, your `tls`."
        );
        assert_eq!(
            line(&hot_desks(&[
                desk(None, "a"),
                desk(Some("Raj"), "b"),
                desk(Some("Andre"), "c"),
                desk(None, "d"),
                desk(None, "e")
            ])),
            "Also in the office now: 5 hot desks, your `a`, Raj's `b`, Andre's `c` and 2 more."
        );
        assert_eq!(
            line(&hot_desks(&[
                desk(None, "a"),
                desk(Some("Raj"), "b"),
                desk(Some("Andre"), "c")
            ])),
            "Also in the office now: 3 hot desks, your `a`, Raj's `b` and Andre's `c`."
        );
    }

    #[test]
    fn unfinished_setup_picks_up_at_the_next_step() {
        let mut facts = office(vec![coworker("John", true)]);
        facts.setup = Some(SetupFact {
            next: 4,
            coworker: Some("John".to_owned()),
        });
        let setup = home(&facts).setup.unwrap();
        assert_eq!(setup.kicker, "Setting up Henrik's office · step 4 of 7");
        assert_eq!(setup.title, "Pick up where you left off: say hi to John");
        assert!(setup.about.starts_with("Say hello, or give John"));
        assert_eq!(setup.button, "Carry on setting up");
        // Done is no step to pick up at.
        facts.setup = Some(SetupFact {
            next: 7,
            coworker: None,
        });
        assert_eq!(home(&facts).setup, None);
    }

    #[test]
    fn without_a_name_the_welcome_stands_alone() {
        let mut facts = office(vec![]);
        facts.person = None;
        assert_eq!(home(&facts).welcome, "Welcome back");
        facts.person = Some("Henrik Lind".to_owned());
        assert_eq!(home(&facts).welcome, "Welcome back, Henrik");
    }

    #[test]
    fn questions_waiting_are_counted_in_words() {
        assert_eq!(waiting(0), None);
        assert_eq!(waiting(1).unwrap(), "One question is waiting for you.");
        assert_eq!(waiting(3).unwrap(), "Three questions are waiting for you.");
    }
}
