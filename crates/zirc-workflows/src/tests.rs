use zirc_proto::card::State as CardState;
use zirc_proto::request::HOLD_SECONDS;

use super::*;

const MASTER: &str = r##"
name = "master"
nick = "master_bot"
description = "fix a broken master"
channel = "#ops"

[[input]]
name = "owner"

[[input]]
name = "why"
optional = true

[roles]
fixers = ["mattias"]

[[on]]
state = "master"
level = "broken"
inputs = { owner = "owner", why = "why" }

[[step]]
id = "fix"
who = "inputs.owner"
says = "Master is broken ({why}): fix it."
outputs = ["cause"]
fallback = "fixers"
nudge = "15m"
escalate = "30m"
stages = [
  { name = "seen", on = "said" },
  { name = "working", on = "push" },
  { name = "landing", on = "landing-queued" },
  { name = "landed", on = "landed" },
]

[[step]]
id = "tell"
kind = "post"
when = "fix.cause == not-mine"
says = "{owner} says it isn't theirs."

[[step]]
id = "green"
kind = "wait"
when = "fix.cause != not-mine"
until = { state = "master", level = "ok" }
nudge = "20m"
"##;

const SPEND: &str = r##"
name = "spend"
nick = "spend_bot"
channel = "#money"

[[input]]
name = "amount"

[[step]]
id = "approve"
kind = "approve"
who = "mattias"
says = "Spend {amount}?"
irreversible = true

[[step]]
id = "say"
kind = "post"
says = "Spending {amount}."
"##;

const MIN: u64 = 60;

fn engine(source: &str) -> Engine {
    Engine::new(parse(source).unwrap(), version(source))
}

fn state(level: Level, owner: Option<&str>) -> Input {
    Input::State(State {
        repo: "#monorepo".into(),
        name: "master".into(),
        level,
        why: owner.map(|_| "//:check fails".into()),
        owner: owner.map(Into::into),
        link: None,
        until: None,
    })
}

fn broken(owner: &str) -> Input {
    state(Level::Broken, Some(owner))
}

fn owner() -> Actor {
    Actor::session("m3-mac", "cc-8b60f7")
}

/// What `by` did, with `detail`, which for a push is its branch.
fn did(by: Actor, did: Did, detail: Option<&str>) -> Input {
    Input::Activity {
        by,
        did,
        detail: detail.map(Into::into),
        branch: (did == Did::Push).then(|| detail.map(Into::into)).flatten(),
        text: None,
    }
}

/// CI's word on a landing of `branch` by `cc-8b60f7`.
fn landing(did: Did, build: &str, branch: &str) -> Input {
    Input::Activity {
        by: Actor::named_session("cc-8b60f7"),
        did,
        detail: Some(format!("build {build}")),
        branch: Some(branch.into()),
        text: None,
    }
}

fn act(by: Actor, run: &str, step: &str, act: Act) -> Input {
    Input::Action {
        by,
        action: StepAction {
            run: run.into(),
            step: step.into(),
            act,
        },
    }
}

fn done(outputs: &[(&str, &str)]) -> Act {
    Act::Done {
        outputs: outputs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
    }
}

/// The run events among `said`.
fn runs(said: &[Said]) -> Vec<RunEvent> {
    said.iter()
        .filter_map(|s| match s {
            Said::Run(event) => RunEvent::of(event),
            Said::Reply(_) => None,
        })
        .collect()
}

fn texts(said: &[Said]) -> Vec<String> {
    said.iter()
        .map(|s| match s {
            Said::Run(event) => event.human_text(),
            Said::Reply(text) => format!("reply: {text}"),
        })
        .collect()
}

/// A master run, broken at 0 and given to `cc-8b60f7`.
fn running() -> Engine {
    let mut e = engine(MASTER);
    e.handle(broken("cc-8b60f7"), 0);
    e
}

#[test]
fn a_broken_master_goes_to_its_owner() {
    let mut e = engine(MASTER);
    let said = e.handle(broken("cc-8b60f7"), 0);
    assert_eq!(
        texts(&said),
        [
            "started `master-1`",
            "`master-1` · `fix` → @cc-8b60f7: Master is broken (//:check fails): fix it. \
             I'll follow along from what you say, push and land; once it lands, hand back \
             with `zirc step done master-1 fix cause=<…>`."
        ]
    );
    let assigned = runs(&said).pop().unwrap();
    assert_eq!(assigned.at, 0);
    assert_eq!(assigned.card.state, CardState::Running);
    assert_eq!(assigned.card.fields[0].value, "cc-8b60f7: given out");
    // CI saying it again while the run goes on starts nothing.
    assert!(e.handle(broken("cc-8b60f7"), 10).is_empty());
}

#[test]
fn the_owner_moves_it_along_by_what_they_do() {
    let mut e = running();
    let mut step =
        |d: Did, detail: Option<&str>, at: u64| texts(&e.handle(did(owner(), d, detail), at));
    assert_eq!(step(Did::Said, None, MIN), ["`master-1` · `fix`: seen"]);
    assert!(step(Did::Said, None, 2 * MIN).is_empty(), "nothing new");
    assert_eq!(
        step(Did::Push, Some("fix-web-unit"), 3 * MIN),
        ["`master-1` · `fix`: working (fix-web-unit)"]
    );
    assert_eq!(
        step(Did::LandingQueued, Some("build 3081"), 4 * MIN),
        ["`master-1` · `fix`: landing (build 3081)"]
    );
    // Landed, a step with outputs waits for them: the fixer's cause= is handed back after.
    assert_eq!(
        step(Did::Landed, Some("build 3081"), 20 * MIN),
        ["`master-1` · `fix`: landed (build 3081)"]
    );
    let back = e.handle(
        act(
            owner(),
            "master-1",
            "fix",
            done(&[("cause", "a stale cache")]),
        ),
        21 * MIN,
    );
    assert_eq!(
        texts(&back),
        [
            "m3-mac/cc-8b60f7 finished `fix` of `master-1`",
            "skipped `tell` of `master-1`",
            "`master-1` waits until master is ok"
        ]
    );
    let green = e.handle(state(Level::Ok, None), 25 * MIN);
    assert_eq!(
        texts(&green),
        ["`green` of `master-1`: it is", "`master-1` is done"]
    );
    assert_eq!(runs(&green)[1].card.state, CardState::Passed);
}

#[test]
fn a_task_with_nothing_to_hand_back_is_done_at_its_last_stage() {
    // The master workflow's fix alone, with nothing to hand back.
    let fix_only = MASTER.split("\n[[step]]\nid = \"tell\"").next().unwrap();
    let source = fix_only.replace("outputs = [\"cause\"]\n", "");
    assert_ne!(
        source, fix_only,
        "the fixture names its outputs as this expects"
    );
    let mut e = engine(&source);
    e.handle(broken("cc-8b60f7"), 0);
    let landed = e.handle(did(owner(), Did::Landed, Some("build 7")), MIN);
    assert_eq!(
        texts(&landed)[0],
        "`master-1` · `fix`: landed (build 7), so it's done"
    );
}

#[test]
fn stages_skip_ahead_and_ignore_everyone_else() {
    let mut e = running();
    let other = Actor::session("other", "cc-123456");
    assert!(e.handle(did(other, Did::Push, Some("x")), 1).is_empty());
    // A landing CI names by its session alone is theirs, from whichever machine.
    let said = e.handle(
        did(
            Actor::named_session("cc-8b60f7"),
            Did::LandingQueued,
            Some("build 9"),
        ),
        2,
    );
    assert_eq!(texts(&said), ["`master-1` · `fix`: landing (build 9)"]);
    assert_eq!(
        runs(&said)[0].card.fields[0].value,
        "cc-8b60f7: landing (build 9)"
    );
}

#[test]
fn a_failed_landing_goes_back_to_working() {
    let mut e = running();
    e.handle(did(owner(), Did::Push, Some("fix-web-unit")), 1);
    e.handle(did(owner(), Did::LandingQueued, Some("build 3081")), 2);
    let failed = e.handle(did(owner(), Did::LandingFailed, Some("build 3081")), 3);
    assert_eq!(
        texts(&failed),
        ["`master-1` · `fix`: landing failed (build 3081), back to working"]
    );
    let again = e.handle(did(owner(), Did::LandingQueued, Some("build 3085")), 4);
    assert_eq!(texts(&again), ["`master-1` · `fix`: landing (build 3085)"]);
}

#[test]
fn a_quiet_step_is_nudged_then_escalated_then_fails() {
    let mut e = running();
    e.handle(did(owner(), Did::Push, Some("fix-web-unit")), MIN);
    assert!(
        e.handle(Input::Tick, 15 * MIN).is_empty(),
        "15 minutes from the push"
    );
    let nudged = e.handle(Input::Tick, 16 * MIN);
    assert_eq!(
        texts(&nudged),
        [
            "`master-1` · `fix` has been quiet for 15m since working (fix-web-unit): \
             @m3-mac/cc-8b60f7, how's it going?"
        ]
    );
    assert_eq!(runs(&nudged)[0].card.state, CardState::Warning);
    assert!(
        e.handle(Input::Tick, 45 * MIN).is_empty(),
        "30 minutes from the nudge"
    );
    let escalated = e.handle(Input::Tick, 46 * MIN);
    assert!(texts(&escalated)[0].starts_with(
        "`master-1` · `fix` hasn't moved since working (fix-web-unit): @mattias, over to you."
    ));
    assert_eq!(
        e.handle(Input::Tick, 61 * MIN).len(),
        1,
        "the fallback is nudged too"
    );
    let failed = e.handle(Input::Tick, 91 * MIN);
    assert_eq!(
        texts(&failed),
        ["`master-1` failed: `fix` failed: stalled since working (fix-web-unit)"]
    );
}

#[test]
fn the_engine_says_when_the_time_next_moves_a_run_on() {
    let mut e = running();
    e.handle(did(owner(), Did::Push, Some("fix-web-unit")), MIN);
    assert_eq!(e.next_due(MIN), Some(16 * MIN), "the nudge");
    e.handle(Input::Tick, 16 * MIN);
    // Nudged: the escalation, 30 minutes on (the nudge's own time passes with nothing).
    assert_eq!(e.next_due(31 * MIN), Some(46 * MIN));
    assert_eq!(
        e.next_due(46 * MIN),
        None,
        "nothing after it, until it's escalated"
    );
}

#[test]
fn anything_they_say_keeps_it_from_being_quiet() {
    let mut e = running();
    e.handle(did(owner(), Did::Said, None), MIN);
    e.handle(did(owner(), Did::Said, None), 14 * MIN);
    assert!(e.handle(Input::Tick, 20 * MIN).is_empty());
    assert_eq!(e.handle(Input::Tick, 29 * MIN).len(), 1);
    // Heard after the nudge, it's quiet again only a whole nudge's time later.
    e.handle(did(owner(), Did::Said, None), 30 * MIN);
    assert!(e.handle(Input::Tick, 44 * MIN).is_empty());
    assert_eq!(e.handle(Input::Tick, 45 * MIN).len(), 1);
}

#[test]
fn a_wait_nudges_whoever_did_the_step_before() {
    let mut e = running();
    e.handle(did(owner(), Did::Landed, Some("build 9")), MIN);
    e.handle(
        act(owner(), "master-1", "fix", done(&[("cause", "x")])),
        MIN,
    );
    let nudged = e.handle(Input::Tick, 21 * MIN);
    assert_eq!(
        texts(&nudged),
        [
            "`master-1` · `green` has been quiet for 20m since it started waiting: \
             @m3-mac/cc-8b60f7, how's it going?"
        ]
    );
}

#[test]
fn a_step_can_still_be_finished_by_hand_with_any_of_its_outputs() {
    let mut e = running();
    let extra = e.handle(act(owner(), "master-1", "fix", done(&[("x", "1")])), 1);
    assert_eq!(
        texts(&extra),
        ["reply: m3-mac/cc-8b60f7: `fix` has no output x: its outputs are cause"]
    );
    let not_mine = e.handle(
        act(owner(), "master-1", "fix", done(&[("cause", "not-mine")])),
        2,
    );
    assert_eq!(
        texts(&not_mine),
        [
            "m3-mac/cc-8b60f7 finished `fix` of `master-1`",
            "cc-8b60f7 says it isn't theirs.",
            "skipped `green` of `master-1`",
            "`master-1` is done"
        ]
    );
    // Without a cause, it waits for green, as a cause that isn't not-mine would.
    let mut e = running();
    let fixed = e.handle(act(owner(), "master-1", "fix", done(&[])), 2);
    assert_eq!(texts(&fixed)[1], "skipped `tell` of `master-1`");
}

#[test]
fn only_those_it_went_to_may_claim_it() {
    let mut e = running();
    let stranger = e.handle(
        act(
            Actor::session("other", "cc-123456"),
            "master-1",
            "fix",
            Act::Claim,
        ),
        1,
    );
    assert_eq!(
        texts(&stranger),
        ["reply: other/cc-123456: `fix` of master-1 is for cc-8b60f7"]
    );
    let claimed = e.handle(act(owner(), "master-1", "fix", Act::Claim), 2);
    assert_eq!(
        texts(&claimed),
        ["m3-mac/cc-8b60f7 took `fix` of `master-1`"]
    );
    assert_eq!(
        runs(&claimed)[0].card.fields[0].value,
        "m3-mac/cc-8b60f7: given out"
    );
}

#[test]
fn an_input_may_name_several_and_an_optional_one_may_be_missing() {
    let mut e = engine(MASTER);
    let said = e.handle(
        Input::Mention {
            by: Actor::person("mattias"),
            text: "run owner=\"m3-mac/cc-1 m3-mac/cc-2\"".into(),
        },
        0,
    );
    assert!(texts(&said)[1].starts_with(
        "`master-1` · `fix` → @m3-mac/cc-1 @m3-mac/cc-2: Master is broken (): fix it."
    ));
    let second = e.handle(did(Actor::session("m3-mac", "cc-2"), Did::Push, None), 1);
    assert_eq!(texts(&second), ["`master-1` · `fix`: working"]);
}

#[test]
fn a_mention_starts_a_run_with_its_inputs_or_says_what_it_needs() {
    let mut e = engine(MASTER);
    let mattias = Actor::person("mattias");
    let mention = |text: &str| Input::Mention {
        by: mattias.clone(),
        text: text.into(),
    };
    assert_eq!(
        texts(&e.handle(mention("run"), 0)),
        ["reply: mattias: it needs owner=…"]
    );
    assert_eq!(
        texts(&e.handle(mention("run colour=red"), 0)),
        ["reply: mattias: \"colour\" isn't one of its inputs (owner; why, optional)"]
    );
    assert!(texts(&e.handle(mention("hello"), 0))[0].contains("I'm the `master` workflow"));
    let said = e.handle(mention("run owner=cc-1a2b3c why=\"a red build\""), 0);
    assert_eq!(texts(&said)[0], "started `master-1` for mattias");
    assert!(texts(&said)[1].contains("Master is broken (a red build)"));
    let cancelled = e.handle(mention("cancel master-1"), 5);
    assert_eq!(
        texts(&cancelled),
        ["`master-1` was cancelled: cancelled by mattias"]
    );
    assert_eq!(
        texts(&e.handle(mention("cancel master-1"), 6)),
        ["reply: mattias: there's no run \"master-1\" going"]
    );
}

#[test]
fn only_a_person_answers_an_approval() {
    let mut e = engine(SPEND);
    let leo = Actor::person("leo");
    e.handle(
        Input::Mention {
            by: leo.clone(),
            text: "run amount=£5".into(),
        },
        0,
    );
    assert_eq!(e.approval("spend-1").as_deref(), Some("approve"));
    let yes = Act::Answer { yes: true };
    let machine = e.handle(
        act(
            Actor::session("mattias", "cc-1"),
            "spend-1",
            "approve",
            yes.clone(),
        ),
        1,
    );
    assert_eq!(
        texts(&machine),
        ["reply: mattias/cc-1: only a person answers an approval"]
    );
    let other = e.handle(act(leo, "spend-1", "approve", yes), 1);
    assert_eq!(
        texts(&other),
        ["reply: leo: `approve` of spend-1 is for mattias"]
    );
}

#[test]
fn an_irreversible_yes_waits_and_a_no_stops_it() {
    let mut e = engine(SPEND);
    e.handle(
        Input::Mention {
            by: Actor::person("leo"),
            text: "run amount=£5".into(),
        },
        0,
    );
    let answer = |yes| {
        act(
            Actor::person("mattias"),
            "spend-1",
            "approve",
            Act::Answer { yes },
        )
    };
    let held = e.handle(answer(true), 10);
    assert!(texts(&held)[0].ends_with("which counts in 60s unless they say no"));
    assert!(e.handle(Input::Tick, 10 + HOLD_SECONDS - 1).is_empty());
    let mut no = e.clone();
    assert_eq!(
        texts(&no.handle(answer(false), 20))[1],
        "`spend-1` was cancelled: mattias said no to `approve`"
    );
    assert_eq!(
        texts(&e.handle(Input::Tick, 10 + HOLD_SECONDS)),
        [
            "the yes to `approve` of `spend-1` counts now",
            "Spending £5.",
            "`spend-1` is done"
        ]
    );
}

#[test]
fn a_run_reads_back_from_its_events_and_what_was_heard() {
    let mut e = engine(MASTER);
    let mut history = runs(&e.handle(broken("cc-8b60f7"), 0));
    history.extend(runs(&e.handle(did(owner(), Did::Push, Some("b")), MIN)));

    let mut back = engine(MASTER);
    for event in &history {
        back.restore(event);
    }
    assert_eq!(back.running(), ["master-1"]);
    // What was heard since, read back too, keeps it from being nudged too soon.
    back.restore_activity(&owner(), 10 * MIN);
    assert!(back.handle(Input::Tick, 20 * MIN).is_empty());
    assert_eq!(back.handle(Input::Tick, 25 * MIN).len(), 1);
    // It carries on from its stage, and numbers the next run after the last.
    let landing = back.handle(did(owner(), Did::LandingQueued, None), 26 * MIN);
    assert_eq!(texts(&landing), ["`master-1` · `fix`: landing"]);
    back.handle(act(owner(), "master-1", "fix", Act::Cancel), 27 * MIN);
    assert_eq!(
        runs(&back.handle(broken("cc-1"), 28 * MIN))[0].run,
        "master-2"
    );
}

#[test]
fn files_that_break_a_rule_say_which() {
    assert!(parse("name = 1").is_err());
    assert!(
        parse(&MASTER.replace("fallback = \"fixers\"", "fallback = \"nobody\""))
            .unwrap_err()
            .contains("isn't a role")
    );
    assert!(
        parse(&format!("{MASTER}\nsurprise = true"))
            .unwrap_err()
            .contains("unknown field")
    );
    assert!(parse(&"#".repeat(MAX_SOURCE + 1)).is_err());
    assert_eq!(version(MASTER).len(), 12);
    assert_ne!(version(MASTER), version(SPEND));
}

#[test]
fn a_run_whose_start_was_not_read_back_still_numbers_the_next() {
    let mut e = running();
    let later = runs(&e.handle(did(owner(), Did::Push, None), MIN));
    let mut back = engine(MASTER);
    back.restore(&later[0]);
    assert!(
        back.running().is_empty(),
        "without its start, it's forgotten"
    );
    assert_eq!(
        runs(&back.handle(broken("cc-1"), 2 * MIN))[0].run,
        "master-2"
    );
}

#[test]
fn only_the_pushed_branch_lands_it() {
    let mut e = running();
    e.handle(did(owner(), Did::Push, Some("fix-web-unit")), 1);
    assert!(
        e.handle(landing(Did::LandingQueued, "3080", "other-work"), 2)
            .is_empty(),
        "their other work"
    );
    let queued = e.handle(landing(Did::LandingQueued, "3081", "fix-web-unit"), 3);
    assert_eq!(texts(&queued), ["`master-1` · `fix`: landing (build 3081)"]);
    assert!(
        e.handle(landing(Did::LandingFailed, "3080", "other-work"), 4)
            .is_empty()
    );
}

#[test]
fn with_several_runs_what_they_do_moves_the_one_it_names() {
    let mut e = engine(MASTER);
    for _ in 0..2 {
        e.handle(
            Input::Mention {
                by: Actor::person("mattias"),
                text: "run owner=cc-8b60f7".into(),
            },
            0,
        );
    }
    let post = |text: &str| Input::Activity {
        by: owner(),
        did: Did::Said,
        detail: None,
        branch: None,
        text: Some(text.into()),
    };
    assert!(
        e.handle(post("looking at the logs"), 1).is_empty(),
        "names neither"
    );
    // master-12 isn't master-1: the id stands whole.
    assert!(
        e.handle(did(owner(), Did::Push, Some("fix-master-12")), 3)
            .is_empty()
    );
    assert_eq!(
        texts(&e.handle(post("**master-2**: found it"), 2)),
        ["`master-2` · `fix`: seen"]
    );
    assert_eq!(
        texts(&e.handle(did(owner(), Did::Push, Some("fix-master-1")), 4)),
        ["`master-1` · `fix`: working (fix-master-1)"]
    );
    let landed = texts(&e.handle(landing(Did::Landed, "3345", "fix-master-1"), 5));
    assert_eq!(landed[0], "`master-1` · `fix`: landed (build 3345)");
    assert!(
        landed.iter().all(|line| !line.contains("master-2")),
        "{landed:?}"
    );
}

#[test]
fn whoever_had_it_may_still_finish_it_after_it_escalates() {
    let mut e = running();
    e.handle(did(owner(), Did::Push, Some("b")), 0);
    e.handle(Input::Tick, 15 * MIN);
    let escalated = e.handle(Input::Tick, 45 * MIN);
    assert_eq!(
        runs(&escalated)[0].change,
        Change::Escalated {
            step: "fix".into(),
            to: vec![
                "cc-8b60f7".into(),
                "m3-mac/cc-8b60f7".into(),
                "mattias".into()
            ],
        }
    );
    let landed = e.handle(did(owner(), Did::Landed, None), 46 * MIN);
    assert_eq!(texts(&landed)[0], "`master-1` · `fix`: landed");
}

#[test]
fn a_card_stays_small_enough_to_draw() {
    // Eight fields of 120 three-byte characters are well past what a card may be.
    let card = Card {
        id: "master-1".into(),
        title: "master-1: fix a broken master".into(),
        state: CardState::Running,
        about: None,
        fields: (0..8)
            .map(|i| Field {
                label: format!("step-{i}"),
                value: "€".repeat(120),
            })
            .collect(),
        links: Vec::new(),
        actions: Vec::new(),
        steps: Vec::new(),
        note: None,
    };
    assert!(card.validate().is_err());
    let fitted = fit(card);
    assert_eq!(fitted.validate(), Ok(()));
    assert_eq!(fitted.fields.len(), 8, "shortened, not dropped");
}

const TRIAGE: &str = r##"
name = "triage"
nick = "triage_bot"
channel = "#ops"

[roles]
leads = ["mattias"]

[[step]]
id = "look"
who = "role:fixers"
says = "Look at it."
nudge = "10m"
escalate = "20m"
fallback = "leads"
"##;

fn member(name: &str, busy: usize, last: u64) -> Member {
    Member {
        name: name.into(),
        busy,
        last,
    }
}

fn fixers(members: Vec<Member>) -> BTreeMap<String, Vec<Member>> {
    BTreeMap::from([("fixers".to_owned(), members)])
}

fn triage() -> Engine {
    let mut e = engine(TRIAGE);
    e.handle(
        Input::Mention {
            by: Actor::person("mattias"),
            text: "run".into(),
        },
        0,
    );
    e
}

#[test]
fn a_role_step_waits_for_its_first_member() {
    let mut e = engine(TRIAGE);
    let said = e.handle(
        Input::Mention {
            by: Actor::person("mattias"),
            text: "run".into(),
        },
        0,
    );
    assert_eq!(
        texts(&said)[1],
        "`triage-1` · `look` waits for someone in fixers: an agent session that runs `zirc \
         role join fixers` gets it"
    );
    assert_eq!(
        runs(&said)[1].card.fields[0].value,
        "waiting for someone in fixers"
    );
    assert!(e.handle(Input::Tick, MIN).is_empty());
    e.set_members(fixers(vec![member("m3-mac/cc-1", 0, 0)]));
    let given = e.handle(Input::Tick, 2 * MIN);
    assert!(texts(&given)[0].starts_with("`triage-1` · `look` → @m3-mac/cc-1: Look at it."));
    assert_eq!(e.holders(), ["m3-mac/cc-1"]);
}

#[test]
fn it_goes_to_the_least_busy_then_the_longest_idle() {
    let mut e = engine(TRIAGE);
    e.set_members(fixers(vec![
        member("a/cc-1", 2, 0),
        member("b/cc-2", 0, 500),
        member("c/cc-3", 0, 100),
    ]));
    let said = e.handle(
        Input::Mention {
            by: Actor::person("mattias"),
            text: "run".into(),
        },
        1000,
    );
    assert!(texts(&said)[1].contains("→ @c/cc-3:"));
}

#[test]
fn a_quiet_member_is_passed_over_then_the_people_have_it() {
    let mut e = triage();
    e.set_members(fixers(vec![member("a/cc-1", 0, 0), member("b/cc-2", 0, 1)]));
    assert!(texts(&e.handle(Input::Tick, 0))[0].contains("→ @a/cc-1:"));
    let next = e.handle(Input::Tick, 10 * MIN);
    assert!(texts(&next)[0].starts_with(
        "`triage-1` · `look` was quiet with @a/cc-1 since it was given out: over to @b/cc-2."
    ));
    // Nobody left to try: b is nudged, then the leads have it.
    let nudged = e.handle(Input::Tick, 20 * MIN);
    assert!(texts(&nudged)[0].contains("@b/cc-2, how's it going?"));
    let escalated = e.handle(Input::Tick, 40 * MIN);
    assert!(texts(&escalated)[0].contains("@mattias, over to you."));
}

#[test]
fn with_no_member_ever_it_goes_to_the_people_in_time() {
    let mut e = triage();
    assert!(e.handle(Input::Tick, 19 * MIN).is_empty());
    let escalated = e.handle(Input::Tick, 20 * MIN);
    assert!(texts(&escalated)[0].contains("@mattias, over to you."));
}

#[test]
fn a_fallback_goes_to_a_live_role_member_and_its_stand_ins_only_without_one() {
    let source = MASTER.replace(
        r#"fixers = ["mattias"]"#,
        r#"fixers = ["role:master-fixers", "mattias"]"#,
    );
    let escalated = |members: Vec<Member>| {
        let mut e = engine(&source);
        e.set_members(BTreeMap::from([("master-fixers".to_owned(), members)]));
        e.handle(broken("cc-8b60f7"), 0);
        e.handle(did(owner(), Did::Push, Some("fix-web-unit")), MIN);
        e.handle(Input::Tick, 16 * MIN);
        texts(&e.handle(Input::Tick, 46 * MIN)).join("\n")
    };
    let to_fixer = escalated(vec![member("m3-mac/master-fixer", 0, 0)]);
    assert!(
        to_fixer.contains("@m3-mac/master-fixer, over to you."),
        "{to_fixer}"
    );
    assert!(!to_fixer.contains("@mattias"), "{to_fixer}");
    let to_mattias = escalated(Vec::new());
    assert!(
        to_mattias.contains("@mattias, over to you."),
        "{to_mattias}"
    );
}

#[test]
fn a_step_that_names_nobody_goes_straight_to_its_fallback() {
    // CI names no owner when too many landings reach what failed (ops/suspects.sh).
    let source = MASTER
        .replace(
            "[[input]]\nname = \"owner\"\n",
            "[[input]]\nname = \"owner\"\noptional = true\n",
        )
        .replace(
            r#"fixers = ["mattias"]"#,
            r#"fixers = ["role:master-fixers", "mattias"]"#,
        );
    let mut e = engine(&source);
    e.set_members(BTreeMap::from([(
        "master-fixers".to_owned(),
        vec![member("m3-mac/master-fixer", 0, 0)],
    )]));
    let said = texts(&e.handle(state(Level::Broken, None), 0)).join("\n");
    assert!(
        said.contains("names nobody: @m3-mac/master-fixer, over to you."),
        "{said}"
    );
    assert!(
        !said.contains("stalled") && !said.contains("nobody to do it"),
        "{said}"
    );
    // Escalated already: quiet past its times, it isn't escalated again, nor failed.
    let later = texts(&e.handle(Input::Tick, 46 * MIN)).join("\n");
    assert!(
        !later.contains("over to you") && !later.contains("failed"),
        "{later}"
    );
}

#[test]
fn a_run_asked_for_again_while_it_goes_on_starts_nothing_new() {
    let source = MASTER.replace(
        "[[input]]\nname = \"owner\"\n",
        "one_run_per = \"why\"\n\n[[input]]\nname = \"owner\"\n",
    );
    let mut e = engine(&source);
    let ci = Actor::person("zirc-ci");
    let mention = |text: &str| Input::Mention {
        by: ci.clone(),
        text: text.into(),
    };
    let first = texts(&e.handle(mention("run owner=cc-1 why=//web:unit"), 0));
    assert_eq!(first[0], "started `master-1` for zirc-ci");
    assert_eq!(
        texts(&e.handle(mention("run owner=cc-2 why=//web:unit"), 1)),
        ["reply: `master-1` is on why=//web:unit already"]
    );
    let other = texts(&e.handle(mention("run owner=cc-2 why=//:check"), 2));
    assert_eq!(other[0], "started `master-2` for zirc-ci");
    e.handle(mention("cancel master-1"), 3);
    let again = texts(&e.handle(mention("run owner=cc-1 why=//web:unit"), 4));
    assert_eq!(again[0], "started `master-3` for zirc-ci");
}

#[test]
fn a_role_step_given_at_start_is_given_once() {
    let mut e = engine(TRIAGE);
    e.set_members(fixers(vec![member("m3-mac/master-fixer", 0, 0)]));
    let started = texts(&e.handle(
        Input::Mention {
            by: Actor::person("zirc-ci"),
            text: "run".into(),
        },
        0,
    ));
    let given = |said: &[String]| said.iter().filter(|t| t.contains(" → ")).count();
    assert_eq!(given(&started), 1, "{started:?}");
    // The member now busy with it, as the live lists say next.
    e.set_members(fixers(vec![member("m3-mac/master-fixer", 1, 0)]));
    let ticked = texts(&e.handle(Input::Tick, 1));
    assert_eq!(given(&ticked), 0, "{ticked:?}");
}

const CHORE: &str = r##"
name = "chore"
nick = "chore_bot"
channel = "#chores"
one_run_per = "what"

[[input]]
name = "what"

[[on]]
every = "1d"
at = "09:00"
with = { what = "update dependencies" }

[[step]]
id = "report"
kind = "post"
says = "Time for: {what}"
"##;

const HOUR: u64 = 3600;
/// A midnight UTC: 2026-11-16.
const DAY0: u64 = 20_773 * 86_400;

#[test]
fn a_time_coming_round_starts_a_run_once_with_what_its_given() {
    let mut e = engine(CHORE);
    // Until its own history is read back it can't tell what it started, and starts nothing.
    assert!(e.handle(Input::Tick, DAY0 + 9 * HOUR).is_empty());
    e.caught_up();
    assert!(e.handle(Input::Tick, DAY0 + 8 * HOUR).is_empty());
    assert_eq!(e.next_due(DAY0 + 8 * HOUR), Some(DAY0 + 9 * HOUR));
    let said = e.handle(Input::Tick, DAY0 + 9 * HOUR);
    assert_eq!(texts(&said)[0], "started `chore-1`");
    assert!(
        texts(&said)
            .iter()
            .any(|t| t.contains("Time for: update dependencies"))
    );
    // Once: the same time again starts nothing, and the next is tomorrow's.
    assert!(e.handle(Input::Tick, DAY0 + 9 * HOUR + MIN).is_empty());
    assert_eq!(e.next_due(DAY0 + 9 * HOUR), Some(DAY0 + 33 * HOUR));
    assert_eq!(
        runs(&e.handle(Input::Tick, DAY0 + 33 * HOUR))[0].run,
        "chore-2"
    );
}

#[test]
fn a_time_the_server_was_away_for_is_passed_over_and_one_read_back_isnt_started_again() {
    let mut e = engine(CHORE);
    e.caught_up();
    // More than an hour late: passed over, for good.
    assert!(e.handle(Input::Tick, DAY0 + 11 * HOUR).is_empty());
    assert!(e.handle(Input::Tick, DAY0 + 11 * HOUR + MIN).is_empty());
    // Within the hour, as after a restart: started.
    let history = runs(&e.handle(Input::Tick, DAY0 + 33 * HOUR + 10 * MIN));
    assert_eq!(history[0].run, "chore-1");

    // Read back by the server that restarted, and by the engine of a deploy again.
    let mut back = engine(CHORE);
    for event in &history {
        back.restore(event);
    }
    back.caught_up();
    assert!(
        back.handle(Input::Tick, DAY0 + 33 * HOUR + 20 * MIN)
            .is_empty()
    );
    let mut again = engine(CHORE);
    again.restore_fired(&e.fired());
    again.caught_up();
    assert!(
        again
            .handle(Input::Tick, DAY0 + 33 * HOUR + 20 * MIN)
            .is_empty()
    );
}

#[test]
fn a_trigger_is_a_state_or_a_time_and_says_whats_wrong_with_it() {
    let wrong = |from: &str, to: &str| parse(&CHORE.replace(from, to)).unwrap_err();
    assert!(
        wrong("every = \"1d\"", "every = \"1d\"\nstate = \"master\"")
            .contains("a trigger has one of")
    );
    assert!(wrong("every = \"1d\"\n", "").contains("a trigger has one of"));
    assert!(wrong("every = \"1d\"", "every = \"1m\"").contains("at least 300s"));
    assert!(wrong("with = { what", "with = { which").contains("isn't an input"));
    assert!(
        wrong("with = { what = \"update dependencies\" }", "")
            .contains("doesn't fill the input \"what\"")
    );
    assert!(wrong("at = \"09:00\"", "level = \"broken\"").contains("only a state has"));
    let state = MASTER.replace("level = \"broken\"", "level = \"broken\"\nat = \"09:00\"");
    assert!(parse(&state).unwrap_err().contains("only every has"));
}

const SUPPORT: &str = r##"
name = "support"
nick = "triage_bot"
channel = "#support"

[[input]]
name = "ask"

[[input]]
name = "from"
optional = true

[[on]]
said = "Refund"
inputs = { ask = "text", from = "by" }

[[on]]
reaction = "🐛"
inputs = { ask = "text", from = "author" }

[[on]]
landed = ["oss/crates/zirc", "MODULE.bazel"]
inputs = { ask = "subject", from = "session" }

[[step]]
id = "note"
kind = "post"
says = "{from}: {ask}"
"##;

fn said(text: &str) -> Input {
    Input::Said {
        by: "ada".into(),
        channel: "#support".into(),
        id: "m1".into(),
        text: text.into(),
    }
}

#[test]
fn words_said_start_a_run_when_theyre_words_of_their_own() {
    let mut e = engine(SUPPORT);
    assert!(e.handle(said("she was refunded already"), 0).is_empty());
    let run = e.handle(said("Can I get a REFUND, please?"), 0);
    assert_eq!(texts(&run)[0], "started `support-1` for ada");
    assert!(
        texts(&run)
            .iter()
            .any(|t| t.contains("ada: Can I get a REFUND, please?"))
    );
}

#[test]
fn a_reaction_starts_a_run_with_the_messages_words_or_says_theyre_gone() {
    let reacted = |reaction: &str, text: Option<&str>| Input::Reacted {
        by: "ada".into(),
        reaction: reaction.into(),
        channel: "#support".into(),
        id: "m1".into(),
        text: text.map(Into::into),
        author: text.map(|_| "guest-1a2b".into()),
    };
    let mut e = engine(SUPPORT);
    assert!(e.handle(reacted("👀", Some("it crashes")), 0).is_empty());
    let run = e.handle(reacted("🐛", Some("it crashes")), 0);
    assert!(
        texts(&run)
            .iter()
            .any(|t| t.contains("guest-1a2b: it crashes"))
    );
    // A message from before it was listening: its words aren't known, and the run needs them.
    let gone = e.handle(reacted("🐛", None), 0);
    assert_eq!(
        texts(&gone),
        ["reply: ada: that starts no run, as its ask isn't known any more"]
    );
}

#[test]
fn a_landing_starts_a_run_when_it_changes_a_path_the_trigger_names() {
    let landed = |changed: &[&str]| Input::Landed {
        landing: BTreeMap::from([
            ("commit".to_owned(), "4b66485".to_owned()),
            ("session".to_owned(), "cc-1a2b3c".to_owned()),
            ("subject".to_owned(), "zirc state says more".to_owned()),
        ]),
        changed: changed.iter().map(|c| (*c).to_owned()).collect(),
    };
    let mut e = engine(SUPPORT);
    assert!(
        e.handle(
            landed(&["web/src/app/page.tsx", "oss/crates/zirc-core/src/lib.rs"]),
            0
        )
        .is_empty()
    );
    assert!(e.handle(landed(&[]), 0).is_empty());
    let run = e.handle(landed(&["oss/crates/zirc/src/state.rs"]), 0);
    assert_eq!(texts(&run)[0], "started `support-1`");
    assert!(
        texts(&run)
            .iter()
            .any(|t| t.contains("cc-1a2b3c: zirc state says more"))
    );
    assert_eq!(
        runs(&e.handle(landed(&["MODULE.bazel"]), 0))[0].run,
        "support-2"
    );
    // A directory CI named for the many files changed in it: anything under it may have.
    assert_eq!(
        runs(&e.handle(landed(&["oss/crates/"]), 0))[0].run,
        "support-3"
    );
}

#[test]
fn the_new_triggers_say_whats_wrong_with_them() {
    let wrong = |from: &str, to: &str| parse(&SUPPORT.replace(from, to)).unwrap_err();
    assert!(wrong("reaction = \"🐛\"", "reaction = \"👍\"").contains("answer an approval"));
    assert!(wrong("\"MODULE.bazel\"", "\"oss/**\"").contains("no patterns"));
    assert!(wrong("said = \"Refund\"", "said = \" \"").contains("some words"));
    assert!(wrong("from = \"author\"", "from = \"owner\"").contains("none of text, author"));
    assert!(
        wrong("said = \"Refund\"", "said = \"Refund\"\nreaction = \"🐛\"")
            .contains("a trigger has one of")
    );
    assert!(wrong("ask = \"subject\", ", "").contains("doesn't fill the input \"ask\""));
}

const ROLLOUT: &str = r##"
name = "rollout"
nick = "rollout_bot"
channel = "#apps"

[[input]]
name = "feature"

[roles]
people = ["mattias"]

[[step]]
id = "build"
each = ["cc-ios", "cc-android"]
says = "{each}: build {feature}"
nudge = "15m"
escalate = "30m"
fallback = "people"
stages = [
  { name = "working", on = "push" },
  { name = "landed", on = "landed" },
]

[[step]]
id = "announce"
kind = "post"
says = "{feature} is on every platform"
"##;

fn session(id: &str) -> Actor {
    Actor::session("m3-mac", id)
}

/// A rollout started by ada at 0: `rollout-1`, its build given to cc-ios and cc-android.
fn rollout(source: &str) -> (Engine, Vec<String>) {
    let mut e = engine(source);
    let said = e.handle(
        Input::Mention {
            by: Actor::person("ada"),
            text: "run feature=reactions".into(),
        },
        0,
    );
    (e, texts(&said))
}

#[test]
fn a_step_for_each_of_several_gives_each_a_run_of_their_own() {
    let (e, said) = rollout(ROLLOUT);
    assert_eq!(said[0], "started `rollout-1` for ada");
    assert!(said[1].starts_with("`rollout-1` · `build` goes to each of cc-ios, cc-android"));
    assert_eq!(
        said[2],
        "started `rollout-1.1`: `build` of `rollout-1` for cc-ios"
    );
    assert!(said[3].starts_with("`rollout-1.1` · `build` → @cc-ios: cc-ios: build reactions"));
    assert!(
        said[5].starts_with("`rollout-1.2` · `build` → @cc-android: cc-android: build reactions")
    );
    assert_eq!(e.running(), ["rollout-1", "rollout-1.1", "rollout-1.2"]);
    let going = e.going();
    assert_eq!((going[0].run.as_str(), going[0].to.len()), ("rollout-1", 0));
    assert_eq!(going[1].to, ["cc-ios"]);
    // The run itself isn't nudged for it: its parts are.
    assert_eq!(e.next_due(0), Some(15 * MIN));
}

#[test]
fn each_part_is_followed_on_its_own_and_the_run_goes_on_when_all_are_done() {
    let (mut e, _) = rollout(ROLLOUT);
    let pushed = e.handle(
        did(session("cc-ios"), Did::Push, Some("ios-reactions")),
        MIN,
    );
    assert_eq!(
        texts(&pushed),
        ["`rollout-1.1` · `build`: working (ios-reactions)"]
    );
    // Quiet, the other is nudged, alone.
    let nudged = texts(&e.handle(Input::Tick, 15 * MIN + 30));
    assert_eq!(nudged.len(), 1);
    assert!(nudged[0].contains("`rollout-1.2`") && nudged[0].contains("@cc-android"));
    let first = texts(&e.handle(
        act(session("cc-ios"), "rollout-1.1", "build", done(&[])),
        17 * MIN,
    ));
    assert!(first.contains(&"`rollout-1.1` is done".to_owned()));
    assert!(!first.iter().any(|t| t.contains("every platform")));
    let last = texts(&e.handle(
        act(session("cc-android"), "rollout-1.2", "build", done(&[])),
        18 * MIN,
    ));
    assert!(last.contains(&"`build` of `rollout-1`: all 2 are done".to_owned()));
    assert!(last.contains(&"reactions is on every platform".to_owned()));
    assert!(e.running().is_empty());
    // Ended, it's listed as one run, from its start to its end: its parts are its own.
    let ended = e.ended();
    assert_eq!(ended.len(), 1);
    assert_eq!(
        (
            ended[0].run.as_str(),
            ended[0].outcome,
            ended[0].started,
            ended[0].ended
        ),
        (
            "rollout-1",
            zirc_proto::run::Outcome::Done,
            Some(0),
            18 * MIN
        )
    );
}

#[test]
fn when_one_fails_the_others_carry_on_unless_the_file_says_stop() {
    let fail = |e: &mut Engine| {
        let why = Act::Fail {
            why: "no simulator".into(),
        };
        texts(&e.handle(act(session("cc-ios"), "rollout-1.1", "build", why), MIN))
    };
    let (mut e, _) = rollout(ROLLOUT);
    fail(&mut e);
    assert_eq!(e.running(), ["rollout-1", "rollout-1.2"]);
    let last = texts(&e.handle(
        act(session("cc-android"), "rollout-1.2", "build", done(&[])),
        2 * MIN,
    ));
    assert!(
        last.contains(
            &"`rollout-1` failed: `build` failed: the parts for cc-ios failed".to_owned()
        )
    );
    assert!(e.running().is_empty());
    // Listed, the step it failed at is apart from why.
    let ended = &e.ended()[0];
    assert_eq!(
        (ended.step.as_deref(), ended.why.as_deref()),
        (Some("build"), Some("the parts for cc-ios failed"))
    );

    let stop = ROLLOUT.replace(
        "channel = \"#apps\"",
        "channel = \"#apps\"\nwhen_one_fails = \"stop\"",
    );
    let (mut e, _) = rollout(&stop);
    let ended = fail(&mut e);
    assert!(
        ended.contains(
            &"`rollout-1` failed: `build` failed: the parts for cc-ios failed".to_owned()
        )
    );
    assert!(ended.contains(&"`rollout-1.2` was cancelled: `rollout-1` ended".to_owned()));
    assert!(e.running().is_empty());
}

#[test]
fn cancelling_the_run_ends_its_parts_and_they_read_back_as_they_were() {
    let (mut e, _) = rollout(ROLLOUT);
    let mut history = Vec::new();
    for said in [
        e.handle(
            Input::Mention {
                by: Actor::person("ada"),
                text: "run feature=threads".into(),
            },
            MIN,
        ),
        e.handle(
            act(session("cc-ios"), "rollout-2.1", "build", done(&[])),
            2 * MIN,
        ),
    ] {
        history.extend(runs(&said));
    }
    let mut back = engine(ROLLOUT);
    for event in &history {
        back.restore(event);
    }
    assert_eq!(back.running(), ["rollout-2", "rollout-2.2"]);
    let last = texts(&back.handle(
        act(session("cc-android"), "rollout-2.2", "build", done(&[])),
        3 * MIN,
    ));
    assert!(last.contains(&"threads is on every platform".to_owned()));

    let cancel = Input::Mention {
        by: Actor::person("ada"),
        text: "cancel rollout-1".into(),
    };
    let ended = texts(&e.handle(cancel, 4 * MIN));
    assert!(ended.contains(&"`rollout-1.1` was cancelled: `rollout-1` ended".to_owned()));
    assert_eq!(e.running(), ["rollout-2", "rollout-2.2"]);
}

#[test]
fn a_step_for_each_reads_its_holders_from_an_input_and_says_whats_wrong_with_it() {
    let from_input = ROLLOUT
        .replace(
            "each = [\"cc-ios\", \"cc-android\"]",
            "each = \"inputs.sessions\"",
        )
        .replace("[roles]", "[[input]]\nname = \"sessions\"\n\n[roles]");
    let mut e = engine(&from_input);
    let text = "run feature=reactions sessions=cc-a,cc-b,cc-a".to_owned();
    e.handle(
        Input::Mention {
            by: Actor::person("ada"),
            text,
        },
        0,
    );
    assert_eq!(e.running(), ["rollout-1", "rollout-1.1", "rollout-1.2"]);

    let wrong = |from: &str, to: &str| parse(&ROLLOUT.replace(from, to)).unwrap_err();
    assert!(wrong("each = [", "who = \"mattias\"\neach = [").contains("in place of a who"));
    assert!(wrong("each = [\"cc-ios\", \"cc-android\"]", "each = []").contains("each of 1 to 8"));
    assert!(wrong("\"cc-android\"]", "\"cc android\"]").contains("none of a nick"));
    assert!(wrong("nudge = \"15m\"", "outputs = [\"commit\"]").contains("no outputs of its own"));
    assert!(wrong("name = \"feature\"", "name = \"each\"").contains("is named \"each\""));
    assert!(wrong("{feature} is on", "{each} is on").contains("isn't an input"));
}
