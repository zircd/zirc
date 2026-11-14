use std::collections::BTreeMap;

use zirc_proto::state::Level;

use super::*;

const MASTER: &str = r##"
name = "master"
nick = "master_bot"
channel = "#ops"

[[input]]
name = "owner"

[[on]]
state = "master"
level = "broken"
inputs = { owner = "owner" }

[[step]]
id = "fix"
who = "inputs.owner"
says = "Master is broken: fix it."
outputs = ["cause"]
stages = [
  { name = "working", on = "push" },
  { name = "landing", on = "landing-queued" },
  { name = "landed", on = "landed" },
]

[[step]]
id = "ship"
kind = "approve"
who = "mattias"
says = "Ship {fix.cause}?"
"##;

/// People are themselves; `m3-mac` is a machine; guests and zirc's own may not act.
fn who(account: &str, session: Option<&str>) -> Option<Actor> {
    match account {
        "m3-mac" => Some(Actor {
            account: account.to_owned(),
            session: session.map(str::to_owned),
            person: false,
        }),
        "guest-abcd" => None,
        a if a.starts_with("zirc") => None,
        a => Some(Actor::person(a)),
    }
}

fn row(channel: &str, account: &str, kind: i64, text: &str) -> Row {
    Row {
        msgid: format!("id-{}", text.len()),
        channel: channel.to_owned(),
        at: 0,
        item: Item {
            kind,
            nick: account.to_owned(),
            account: account.to_owned(),
            text: text.to_owned(),
            tags: BTreeMap::new(),
        },
    }
}

fn zirc(channel: &str, account: &str, session: &str, event: Event) -> Row {
    let body = zirc_proto::encode(&Message::new(session, event)).unwrap();
    row(channel, account, PRIVMSG, &body)
}

fn deploy(account: &str, source: &str) -> Row {
    let event = Deploy {
        name: "master".into(),
        nick: "master_bot".into(),
        source: source.into(),
    }
    .event();
    // A deploy is a multiline message, which the history keeps joined.
    let message = Message::new("cc-1", event);
    let body = zirc_proto::encode_lines_up_to(&message, 16_384).unwrap();
    row("#workflows", account, PRIVMSG, &body)
}

fn broken(owner: &str) -> Event {
    State {
        repo: "#monorepo".into(),
        name: "master".into(),
        level: Level::Broken,
        why: None,
        owner: Some(owner.into()),
        link: None,
        until: None,
    }
    .event()
}

/// Each post's nick, channel and text.
fn said(outs: &[Out]) -> Vec<(String, String, String)> {
    outs.iter()
        .filter_map(|out| match out {
            Out::Post {
                nick,
                channel,
                message,
            } => Some((nick.clone(), channel.clone(), message.event.human_text())),
            _ => None,
        })
        .collect()
}

/// The posts as history rows, as Ergo would keep them, from the workflow's account.
fn kept(outs: &[Out]) -> Vec<Row> {
    outs.iter()
        .filter_map(|out| match out {
            Out::Post {
                nick,
                channel,
                message,
            } => {
                let body = zirc_proto::encode_lines_up_to(message, 16_384).unwrap();
                Some(row(channel, nick, PRIVMSG, &body))
            }
            _ => None,
        })
        .collect()
}

fn installed(hosted: &mut Hosted, outs: Vec<Out>) -> Vec<Out> {
    let mut rest = Vec::new();
    for out in outs {
        match out {
            Out::Check(candidate) => {
                rest.push(Hosted::deployed(&candidate));
                rest.extend(hosted.install(candidate));
            }
            other => rest.push(other),
        }
    }
    rest
}

#[test]
fn a_deploy_is_checked_then_runs_as_its_nick() {
    let mut hosted = Hosted::default();
    assert!(
        hosted
            .take(&deploy("guest-abcd", MASTER), true, &who, 0)
            .is_empty(),
        "a guest deploys nothing"
    );
    let bad = hosted.take(&deploy("mattias", "name = 1"), true, &who, 0);
    assert_eq!(said(&bad)[0].0, workflow::HOST);
    assert!(
        said(&bad)[0]
            .2
            .starts_with("mattias: couldn't deploy `master`: ")
    );

    let outs = hosted.take(&deploy("m3-mac", MASTER), true, &who, 0);
    assert!(matches!(outs.as_slice(), [Out::Check(_)]));
    let outs = installed(&mut hosted, outs);
    assert!(said(&outs)[0].2.starts_with("deployed `master` ("));
    assert!(outs.iter().any(
        |o| matches!(o, Out::Bot { nick, channels } if nick == "master_bot" && channels == &["#ops"])
    ));
    assert_eq!(hosted.count(), 1);
    // Always connected, it's zirc's own, as the host and the recorder are: nobody's account.
    assert!(is_zircs_own("master_bot") && is_zircs_own("zirc-workflows"));
    assert!(!is_zircs_own("mattias"));

    // A broken master, said by CI's account in a repository's channel, starts a run.
    let run = hosted.take(
        &zirc("#monorepo", "zirc-ci", "ci", broken("cc-8b60f7")),
        true,
        &who,
        10,
    );
    let posts = said(&run);
    assert_eq!(
        posts[0],
        (
            "master_bot".into(),
            "#ops".into(),
            "started `master-1`".into()
        )
    );
    assert!(posts[1].2.contains("@cc-8b60f7"));
}

#[test]
fn workers_claim_and_finish_steps_and_approvers_react() {
    let mut hosted = Hosted::default();
    let outs = hosted.take(&deploy("mattias", MASTER), true, &who, 0);
    installed(&mut hosted, outs);
    let started = hosted.take(
        &zirc("#ops", "zirc-ci", "ci", broken("cc-8b60f7")),
        true,
        &who,
        1,
    );
    for row in kept(&started) {
        assert!(
            hosted.take(&row, true, &who, 1).is_empty(),
            "its own posts start nothing"
        );
    }

    let claim = StepAction {
        run: "master-1".into(),
        step: "fix".into(),
        act: Act::Claim,
    };
    let claimed = hosted.take(
        &zirc("#ops", "m3-mac", "cc-8b60f7", claim.event()),
        true,
        &who,
        2,
    );
    assert_eq!(
        said(&claimed)[0].2,
        "m3-mac/cc-8b60f7 took `fix` of `master-1`"
    );

    let done = StepAction {
        run: "master-1".into(),
        step: "fix".into(),
        act: Act::Done {
            outputs: BTreeMap::from([("cause".into(), "a typo".into())]),
        },
    };
    let fixed = hosted.take(
        &zirc("#ops", "m3-mac", "cc-8b60f7", done.event()),
        true,
        &who,
        3,
    );
    let posts = said(&fixed);
    assert!(
        posts[1]
            .2
            .starts_with("`master-1` · `ship` → @mattias: Ship a typo?")
    );

    // The approval's message, once in the history, takes a 👍 from the approver alone.
    // Whichever of the run's messages the card is on, a reaction there answers it.
    let mut ask = kept(&fixed).remove(0);
    ask.msgid = "ask-1".into();
    hosted.take(&ask, true, &who, 3);
    let react = |account: &str, emoji: &str| {
        let mut r = row("#ops", account, TAGMSG, "");
        r.item.tags = BTreeMap::from([
            ("+draft/react".into(), emoji.into()),
            ("+draft/reply".into(), "ask-1".into()),
        ]);
        r
    };
    let leo = hosted.take(&react("leo", "👍"), true, &who, 4);
    assert_eq!(said(&leo)[0].2, "leo: `ship` of master-1 is for mattias");
    assert!(
        hosted
            .take(&react("mattias", "🎉"), true, &who, 4)
            .is_empty()
    );
    let yes = hosted.take(&react("mattias", "👍"), true, &who, 5);
    assert_eq!(
        said(&yes).iter().map(|p| p.2.as_str()).collect::<Vec<_>>(),
        [
            "mattias said yes to `ship` of `master-1`",
            "`master-1` is done"
        ]
    );
}

#[test]
fn a_mention_starts_a_run_from_any_client() {
    let mut hosted = Hosted::default();
    let outs = hosted.take(&deploy("mattias", MASTER), true, &who, 0);
    installed(&mut hosted, outs);
    let plain = hosted.take(
        &row("#ops", "mattias", PRIVMSG, "master_bot: run owner=cc-1"),
        true,
        &who,
        1,
    );
    assert_eq!(said(&plain)[0].2, "started `master-1` for mattias");
    let app = zirc(
        "#ops",
        "mattias",
        "mattias",
        Event::Text {
            body: "@master_bot run owner=cc-2".into(),
        },
    );
    assert_eq!(
        said(&hosted.take(&app, true, &who, 2))[0].2,
        "started `master-2` for mattias"
    );
    assert!(
        hosted
            .take(
                &row("#elsewhere", "mattias", PRIVMSG, "master_bot: run owner=x"),
                true,
                &who,
                3
            )
            .is_empty(),
        "only in its channel"
    );
    assert!(
        hosted
            .take(
                &row("#ops", "mattias", PRIVMSG, "master_botty run"),
                true,
                &who,
                3
            )
            .is_empty()
    );
}

#[test]
fn a_guests_state_starts_no_run_and_cis_and_a_machines_do() {
    let mut hosted = Hosted::default();
    let outs = hosted.take(&deploy("mattias", MASTER), true, &who, 0);
    installed(&mut hosted, outs);
    let guests = zirc("#ops", "guest-abcd", "web", broken("cc-8b60f7"));
    assert!(hosted.take(&guests, true, &who, 1).is_empty());
    // Read back when the sidecar starts, it's no state either.
    assert!(hosted.take(&guests, false, &who, 1).is_empty());
    let machines = zirc("#ops", "m3-mac", "cc-1", broken("cc-8b60f7"));
    assert!(!said(&hosted.take(&machines, true, &who, 2)).is_empty());
}

#[test]
fn an_integrations_event_naming_the_workflow_starts_a_run_and_a_guests_doesnt() {
    let mut hosted = Hosted::default();
    let outs = hosted.take(&deploy("mattias", MASTER), true, &who, 0);
    installed(&mut hosted, outs);
    let flaky = |account: &str| {
        zirc(
            "#ops",
            account,
            "ci",
            Event::Custom {
                kind: "x-zirc/flaky".into(),
                data: serde_json::json!({}),
                text: "@master_bot run owner=cc-1".into(),
            },
        )
    };
    assert_eq!(
        said(&hosted.take(&flaky("zirc-ci"), true, &who, 1))[0].2,
        "started `master-1` for zirc-ci"
    );
    assert!(hosted.take(&flaky("guest-abcd"), true, &who, 2).is_empty());
    assert!(hosted.take(&flaky("m3-mac"), true, &who, 2).is_empty());
}

#[test]
fn the_list_has_each_workflow_its_file_and_where_its_runs_stand() {
    let mut hosted = Hosted::default();
    assert!(hosted.listed().is_empty());
    deploys(&mut hosted, "mattias", MASTER);
    let listed = hosted.listed();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0].name.as_str(), listed[0].nick.as_str()),
        ("master", "master_bot")
    );
    assert_eq!(listed[0].by.as_deref(), Some("mattias"));
    assert_eq!(listed[0].source, MASTER);
    assert_eq!(listed[0].version, zirc_workflows::version(MASTER));
    assert!(listed[0].runs.is_empty());

    hosted.take(
        &zirc("#monorepo", "zirc-ci", "ci", broken("cc-8b60f7")),
        true,
        &who,
        10,
    );
    let runs = &hosted.listed()[0].runs;
    assert_eq!(runs.len(), 1);
    assert_eq!(
        (runs[0].run.as_str(), runs[0].step.as_str()),
        ("master-1", "fix")
    );
    assert_eq!(runs[0].to, ["cc-8b60f7"]);
    assert_eq!(runs[0].since, Some(10));
}

/// What the host says of a deploy of `source` by `account`, heard live.
fn deploys(hosted: &mut Hosted, account: &str, source: &str) -> String {
    let outs = hosted.take(&deploy(account, source), true, &who, 0);
    let outs = installed(hosted, outs);
    said(&outs)[0].2.clone()
}

#[test]
fn a_workflow_is_replaced_by_its_deployer_an_owner_or_an_admin_and_not_under_a_run() {
    let mut hosted = Hosted::default();
    hosted.know(
        ["ada".to_owned()].into(),
        [("m3-mac".to_owned(), "mattias".to_owned())].into(),
    );
    assert!(deploys(&mut hosted, "mattias", MASTER).starts_with("deployed `master`"));
    let other = MASTER.replace("Ship {fix.cause}?", "Ship it?");
    // Another member, or their machine, doesn't replace it; an admin, the deployer and the
    // deployer's machine do.
    let refused = deploys(&mut hosted, "bob", &other);
    assert!(refused.contains("is @mattias's"), "{refused}");
    for may in ["ada", "mattias", "m3-mac"] {
        let done = deploys(&mut hosted, may, &other);
        assert!(done.starts_with("deployed `master`"), "{may}: {done}");
    }
    // With a run going, nobody does: it would go on by other steps than it started with.
    let broke = zirc("#ops", "zirc-ci", "ci", broken("cc-8b60f7"));
    assert!(!hosted.take(&broke, true, &who, 1).is_empty());
    let held = deploys(&mut hosted, "mattias", MASTER);
    assert!(held.contains("has master-1 going"), "{held}");
}

#[test]
fn a_deploy_the_host_refused_stays_refused_when_read_back() {
    let mut first = Hosted::default();
    let deployed = deploy("mattias", MASTER);
    let outs = first.take(&deployed, true, &who, 0);
    // Refused when it was said, as when its nick is someone's account: no word that it was
    // deployed follows it in the history.
    assert!(matches!(outs.as_slice(), [Out::Check(_)]));
    let mut again = Hosted::default();
    assert!(again.take(&deployed, false, &who, 100).is_empty());
    assert_eq!(again.count(), 0);
    // Deployed, its word follows, and reading both back installs it.
    let acked = kept(&installed(&mut first, outs));
    let read = |row| again.take(row, false, &who, 100);
    let bots: Vec<Out> = acked.iter().flat_map(read).collect();
    assert!(matches!(bots.as_slice(), [Out::Bot { .. }]));
    assert_eq!(again.count(), 1);
}

#[test]
fn a_restart_reads_it_all_back_and_says_nothing_old_again() {
    let mut first = Hosted::default();
    let mut history = vec![deploy("mattias", MASTER)];
    let outs = first.take(&history[0], true, &who, 0);
    // The host's word that it deployed it is kept too: read back, it's what installs it.
    history.extend(kept(&installed(&mut first, outs)));
    let state = zirc("#ops", "zirc-ci", "ci", broken("cc-8b60f7"));
    let started = first.take(&state, true, &who, 1);
    history.push(state);
    history.extend(kept(&started));

    let mut again = Hosted::default();
    let mut outs = Vec::new();
    for row in &history {
        outs.extend(again.take(row, false, &who, 100));
    }
    assert!(said(&outs).is_empty(), "nothing old is said again");
    assert!(outs.iter().any(|o| matches!(o, Out::Bot { .. })));
    // The run goes on where it was: CI saying it again starts nothing new, and the owner
    // may claim the step.
    assert!(
        said(&again.take(
            &zirc("#ops", "zirc-ci", "ci", broken("cc-8b60f7")),
            true,
            &who,
            101
        ))
        .is_empty()
    );
    let claim = StepAction {
        run: "master-1".into(),
        step: "fix".into(),
        act: Act::Claim,
    };
    let claimed = again.take(
        &zirc("#ops", "m3-mac", "cc-8b60f7", claim.event()),
        true,
        &who,
        102,
    );
    assert_eq!(
        said(&claimed)[0].2,
        "m3-mac/cc-8b60f7 took `fix` of `master-1`"
    );
}

#[test]
fn addressing_takes_the_usual_forms() {
    assert_eq!(
        addressed("@master_bot run x=1", "master_bot"),
        Some("run x=1")
    );
    assert_eq!(addressed("master_bot: run", "master_bot"), Some("run"));
    assert_eq!(
        addressed("Master_Bot, cancel master-1", "master_bot"),
        Some("cancel master-1")
    );
    assert_eq!(addressed("master_bot", "master_bot"), Some(""));
    assert_eq!(addressed("master_bots run", "master_bot"), None);
    assert_eq!(addressed("hi master_bot", "master_bot"), None);
}

#[test]
fn what_the_owner_does_and_what_ci_says_moves_their_task() {
    let mut hosted = Hosted::default();
    let outs = hosted.take(&deploy("mattias", MASTER), true, &who, 0);
    installed(&mut hosted, outs);
    hosted.take(
        &zirc("#ops", "zirc-ci", "ci", broken("cc-8b60f7")),
        true,
        &who,
        1,
    );
    let push = Event::Push {
        branch: "fix-web-unit".into(),
        head: "1a2b3c4".into(),
        forced: false,
        subjects: Vec::new(),
        paths: Vec::new(),
        base: None,
        lines: BTreeMap::new(),
        repo: None,
    };
    let pushed = hosted.take(
        &zirc("#monorepo", "m3-mac", "cc-8b60f7", push),
        true,
        &who,
        2,
    );
    assert_eq!(
        said(&pushed)[0].2,
        "`master-1` · `fix`: working (fix-web-unit)"
    );
    let landing = Event::Custom {
        kind: zirc_proto::ci::LANDING_KIND.into(),
        data: serde_json::json!({"session": "cc-8b60f7", "build": "3081", "state": "running"}),
        text: "landing".into(),
    };
    let queued = hosted.take(
        &zirc("#monorepo/landings", "zirc-ci", "ci", landing),
        true,
        &who,
        3,
    );
    assert_eq!(
        said(&queued)[0].2,
        "`master-1` · `fix`: landing (build 3081)"
    );
    // Only CI's account speaks for a landing.
    let forged = Event::Custom {
        kind: zirc_proto::ci::LANDING_KIND.into(),
        data: serde_json::json!({"session": "cc-8b60f7", "build": "1", "state": "passed"}),
        text: "landed".into(),
    };
    assert!(said(&hosted.take(&zirc("#monorepo", "leo", "leo", forged), true, &who, 4)).is_empty());
}

#[test]
fn a_finished_run_keeps_only_its_end_which_numbers_the_next() {
    let mut hosted = Hosted::default();
    let outs = hosted.take(&deploy("mattias", MASTER), true, &who, 0);
    installed(&mut hosted, outs);
    let started = hosted.take(
        &zirc(
            "#ops",
            "mattias",
            "mattias",
            Event::Text {
                body: "@master_bot run owner=cc-1".into(),
            },
        ),
        true,
        &who,
        1,
    );
    let cancel = hosted.take(
        &zirc(
            "#ops",
            "mattias",
            "mattias",
            Event::Text {
                body: "@master_bot cancel master-1".into(),
            },
        ),
        true,
        &who,
        2,
    );
    for row in kept(&started).iter().chain(&kept(&cancel)) {
        hosted.take(row, true, &who, 2);
    }
    assert_eq!(hosted.logs["master"].len(), 1);
    assert!(hosted.messages.is_empty());
    // Deployed again, it carries on numbering from the last.
    let outs = hosted.take(&deploy("mattias", MASTER), true, &who, 3);
    installed(&mut hosted, outs);
    let next = hosted.take(
        &zirc(
            "#ops",
            "mattias",
            "mattias",
            Event::Text {
                body: "@master_bot run owner=cc-2".into(),
            },
        ),
        true,
        &who,
        4,
    );
    assert_eq!(said(&next)[0].2, "started `master-2` for mattias");
}

const TRIAGE: &str = r##"
name = "triage"
nick = "triage_bot"
channel = "#ops"

[[step]]
id = "look"
who = "role:fixers"
says = "Look at it."
nudge = "10m"
"##;

/// What machine `nick` says of its live sessions and their roles, in the organisation's
/// channel, as the host's connection hears it.
fn live_list(nick: &str, roles: &[(&str, &str)]) -> zirc_irc::Line {
    let list = zirc_proto::session::Live {
        sessions: roles.iter().map(|(s, _)| (*s).to_owned()).collect(),
        roles: roles
            .iter()
            .map(|(s, r)| ((*s).to_owned(), vec![(*r).to_owned()]))
            .collect(),
    };
    let mut line = zirc_irc::Line::new("TAGMSG", [zirc_proto::ORG_CHANNEL]);
    line.source = Some(format!("{nick}!u@h"));
    line.tags = vec![(
        zirc_proto::EVENT_TAG.into(),
        Some(zirc_proto::payload(&Message::new("watcher", list.event())).unwrap()),
    )];
    line
}

fn triage_deploy() -> Row {
    let event = Deploy {
        name: "triage".into(),
        nick: "triage_bot".into(),
        source: TRIAGE.into(),
    }
    .event();
    let body = zirc_proto::encode_lines_up_to(&Message::new("cc-1", event), 16_384).unwrap();
    row("#workflows", "mattias", PRIVMSG, &body)
}

#[test]
fn a_role_step_goes_to_a_member_the_live_lists_name() {
    let mut hosted = Hosted::default();
    let outs = hosted.take(&triage_deploy(), true, &who, 0);
    installed(&mut hosted, outs);
    let run = row("#ops", "mattias", PRIVMSG, "triage_bot: run");
    let started = hosted.take(&run, true, &who, 1);
    assert!(said(&started)[1].2.contains("waits for someone in fixers"));
    for row in kept(&started) {
        hosted.take(&row, true, &who, 1);
    }
    hosted.heard(&live_list("m3-mac", &[("cc-1", "fixers")]));
    let given = hosted.tick(2);
    assert!(
        said(&given)[0]
            .2
            .starts_with("`triage-1` · `look` → @m3-mac/cc-1:")
    );
    for row in kept(&given) {
        hosted.take(&row, true, &who, 2);
    }
    // Gone from the channel, its sessions are nobody's to pass a quiet step on to.
    let mut quit = zirc_irc::Line::new("QUIT", ["bye"]);
    quit.source = Some("m3-mac!u@h".into());
    hosted.heard(&quit);
    assert!(hosted.members().is_empty());
    hosted.heard(&live_list("other", &[("cc-2", "fixers")]));
    let passed = hosted.tick(11 * 60);
    assert!(said(&passed)[0].2.contains("over to @other/cc-2"));
}

/// A small workflow that posts once.
const HELLO: &str = r##"
name = "hello"
nick = "hello_bot"
channel = "#ops"

[[step]]
id = "greet"
kind = "post"
says = "hello from the workflow"
"##;

/// A host on `ergo` with workflows on, keeping what it keeps in `dir`.
fn host_on(ergo: &zirc_test_ergo::Ergo, dir: &std::path::Path) -> Host {
    let gateway = crate::gateway::Gateway::new("127.0.0.1:1".parse().unwrap(), None, None, None);
    gateway.set(crate::gateway::Public {
        workflows: true,
        ..crate::gateway::Public::default()
    });
    Host {
        host: "127.0.0.1".into(),
        port: ergo.port(),
        gate: None,
        token: "token".into(),
        history: dir.join("history.db"),
        machines: Machines::default(),
        gateway,
        admins: Box::new(|| Box::pin(async { std::collections::BTreeSet::new() })),
    }
}

/// A person's login on `ergo` as `nick`, their account registered.
async fn person_on(ergo: &zirc_test_ergo::Ergo, nick: &str) -> zirc_irc::Config {
    zirc_test_irc::register(ergo, nick, "a-passphrase").await;
    let mut config = zirc_test_irc::config(ergo, nick);
    config.sasl = Some(zirc_irc::Credentials {
        account: nick.into(),
        password: "a-passphrase".into(),
    });
    config
}

/// Whether a line is `nick` saying `words` in `channel`.
fn said_by(
    nick: &'static str,
    channel: &'static str,
    words: &'static str,
) -> impl Fn(&zirc_irc::Line) -> bool {
    move |line| {
        line.command == "PRIVMSG"
            && line.source.as_deref().is_some_and(|s| s.starts_with(nick))
            && line.params[0] == channel
            && line.params[1].contains(words)
    }
}

/// Whether a line is `nick` joining `channel`.
fn joined(nick: &'static str, channel: &'static str) -> impl Fn(&zirc_irc::Line) -> bool {
    move |line| {
        line.command == "JOIN"
            && line.source.as_deref().is_some_and(|s| s.starts_with(nick))
            && line.params[0].eq_ignore_ascii_case(channel)
    }
}

#[tokio::test]
async fn a_deployed_workflow_hears_live_where_it_is_and_where_its_invited() {
    use zirc_test_irc::{joins, next_matching};
    let ergo = zirc_test_ergo::require_ergo!();
    let dir = tempfile::tempdir().unwrap();
    let host = host_on(&ergo, dir.path());
    let (ergo, port) = (&ergo, ergo.port());
    // Each nick's account, as the sidecar makes it: registered with its derived passphrase.
    let account = move |nick: String| async move {
        let config = crate::say::config("127.0.0.1", port, None, "token", &nick);
        zirc_test_irc::register(ergo, &nick, &config.sasl.unwrap().password).await;
        true
    };
    let config = person_on(ergo, "ada").await;
    // In before the host, so she hears it come.
    let (ada, mut hears) = zirc_irc::connect(&config).await.unwrap();
    joins(&ada, &mut hears, "#workflows").await;
    joins(&ada, &mut hears, "#ops").await;
    let ada = async {
        next_matching(&mut hears, joined("zirc-workflows", "#workflows")).await;
        // Deployed: it says so, and its nick comes to its channel.
        let deploy = Deploy {
            name: "hello".into(),
            nick: "hello_bot".into(),
            source: HELLO.into(),
        };
        crate::say::send(&ada, "#workflows", &Message::new("ada", deploy.event())).unwrap();
        next_matching(
            &mut hears,
            said_by("zirc-workflows", "#workflows", "deployed"),
        )
        .await;
        next_matching(&mut hears, joined("hello_bot", "#ops")).await;
        // Invited, it comes.
        joins(&ada, &mut hears, "#elsewhere").await;
        ada.send(zirc_irc::Line::new("INVITE", ["hello_bot", "#elsewhere"]))
            .unwrap();
        next_matching(&mut hears, joined("hello_bot", "#elsewhere")).await;
        // Mentioned, it runs.
        ada.privmsg("#ops", "@hello_bot run").unwrap();
        next_matching(
            &mut hears,
            said_by("hello_bot", "#ops", "hello from the workflow"),
        )
        .await;
    };
    tokio::select! {
        () = host.run(account) => panic!("the host stopped"),
        () = ada => {}
    }
    // Where it was invited is kept for next time.
    let kept = std::fs::read_to_string(dir.path().join(KEPT)).unwrap();
    assert!(kept.contains("#elsewhere"), "{kept}");
}

#[test]
fn a_role_step_a_member_is_live_for_is_given_once_when_its_post_is_heard_back() {
    let mut hosted = Hosted::default();
    let outs = hosted.take(&triage_deploy(), true, &who, 0);
    installed(&mut hosted, outs);
    hosted.heard(&live_list("m3-mac", &[("master-fixer", "fixers")]));
    hosted.tick(0);
    let run = row("#ops", "mattias", PRIVMSG, "triage_bot: run");
    let started = hosted.take(&run, true, &who, 1);
    let given = |outs: &[Out]| {
        said(outs)
            .iter()
            .filter(|(_, _, text)| text.contains(" → "))
            .count()
    };
    assert_eq!(given(&started), 1, "{:?}", said(&started));
    // Its own posts, heard back as the server echoes them, give it to nobody again.
    let mut again = Vec::new();
    for row in kept(&started) {
        again.extend(hosted.take(&row, true, &who, 1));
    }
    again.extend(hosted.tick(1));
    assert_eq!(given(&again), 0, "{:?}", said(&again));
}

#[test]
fn cis_mention_of_a_role_step_gives_it_once() {
    let mut hosted = Hosted::default();
    let outs = hosted.take(&triage_deploy(), true, &who, 0);
    installed(&mut hosted, outs);
    hosted.heard(&live_list("m3-mac", &[("master-fixer", "fixers")]));
    hosted.tick(0);
    let flaky = zirc(
        "#ops",
        "zirc-ci",
        "ci",
        Event::Custom {
            kind: "x-zirc/check".into(),
            data: serde_json::json!({"state": "warning"}),
            text: "@triage_bot run".into(),
        },
    );
    // As the host's loop does: what it heard, then a tick, in one go.
    let mut started = hosted.take(&flaky, true, &who, 1);
    started.extend(hosted.tick(1));
    // Then each of its posts read back, as a reconnect's catch-up does, and the member's
    // machine saying its list again, each followed by a tick, as the loop does.
    let echoes = kept(&started);
    let mut all = started;
    for row in echoes {
        all.extend(hosted.take(&row, false, &who, 1));
        all.extend(hosted.tick(1));
    }
    hosted.heard(&live_list("m3-mac", &[("master-fixer", "fixers")]));
    all.extend(hosted.tick(1));
    let texts: Vec<String> = said(&all).into_iter().map(|(_, _, t)| t).collect();
    assert_eq!(
        texts.iter().filter(|t| t.contains(" → ")).count(),
        1,
        "{texts:?}"
    );
}
