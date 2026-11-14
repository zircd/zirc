//! Posts, channels, topics and the agents room.

use tempfile::TempDir;
use zirc_core::ipc::{self, Request, Response, Target, Why};
use zirc_proto::Event;
use zirc_test_ergo::git::git;
use zirc_test_ergo::require_ergo;

mod common;
use common::*;

#[tokio::test]
async fn a_watched_repository_has_its_agents_and_landings_rooms_from_the_first_day() {
    let ergo = require_ergo!();
    let [(alpha, _alpha_clone), _] = two_machines(&ergo).await;
    let joined = |channel: &'static str| {
        move |status: &ipc::Status| status.channels.iter().any(|c| c == channel)
    };
    for channel in ["#api/agents", "#api/landings"] {
        alpha
            .eventually(joined(channel), &format!("joining {channel}"))
            .await;
    }
}

#[tokio::test]
async fn branches_and_topics_have_channels_of_their_own() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    let mut beta_hears = beta.watch(None).await;
    let joined = |channel: &'static str| {
        move |status: &ipc::Status| status.channels.iter().any(|c| c == channel)
    };

    let post = |text: &str, to: Target| plain_post(alpha_clone.path(), "cc-a", text, to);
    // An unpushed branch, such as a session's own, has no channel: posting to it is refused.
    git(alpha_clone.path(), &["checkout", "--quiet", "-b", "login"]);
    let refused = ipc::request(&alpha.socket(), &post("anyone?", Target::Branch)).await;
    assert!(refused.unwrap_err().to_string().contains("no channel"));

    // Topics are joined by choice, per repository or across the organisation; a branch's
    // channel is a topic named for the branch.
    let topic = |repo: &TempDir, topic: &str, org: bool| Request::Join {
        repo: repo.path().to_owned(),
        topic: topic.into(),
        org,
        session: None,
    };

    // Pushed, the login branch has a channel, but nobody is in it unless they join: a
    // landing pushes every branch it lands, and each would otherwise make a channel.
    git(beta_clone.path(), &["checkout", "--quiet", "-b", "login"]);
    for clone in [&alpha_clone, &beta_clone] {
        git(
            clone.path(),
            &["update-ref", "refs/remotes/origin/login", "HEAD"],
        );
    }
    // Once alpha has seen the push, posting to the branch says how to join its channel.
    until_refused(
        &alpha,
        &post("anyone?", Target::Branch),
        "`zirc join login`",
    )
    .await;
    for (machine, clone) in [(&alpha, &alpha_clone), (&beta, &beta_clone)] {
        machine.request(topic(clone, "login", false)).await;
        machine
            .eventually(joined("#api/login"), "joining #api/login")
            .await;
    }

    // A post goes to the repository unless told otherwise.
    let posted = alpha
        .request(post("to everyone on api", Target::Repo))
        .await;
    assert_eq!(posted, Response::channel("#api"));
    assert_eq!(next_post(&mut beta_hears).await.channel, "#api");
    alpha.request(post("on login", Target::Branch)).await;
    assert_eq!(next_post(&mut beta_hears).await.channel, "#api/login");
    alpha.request(post("everyone", Target::Org)).await;
    assert_eq!(next_post(&mut beta_hears).await.channel, "#general");

    for (machine, clone) in [(&alpha, &alpha_clone), (&beta, &beta_clone)] {
        machine.request(topic(clone, "auth", false)).await;
        machine
            .eventually(joined("#api/auth"), "joining #api/auth")
            .await;
    }
    let to_auth = Target::Topic {
        name: "auth".into(),
        org: false,
    };
    alpha.request(post("tokens rotate hourly", to_auth)).await;
    let said = next_post(&mut beta_hears).await;
    assert_eq!(said.channel, "#api/auth");
    assert_eq!(
        alpha.request(topic(&alpha_clone, "releases", true)).await,
        Response::channel("#releases")
    );
    // A topic that isn't joined can't be posted to.
    let not_joined = ipc::request(
        &alpha.socket(),
        &post(
            "hello?",
            Target::Topic {
                name: "billing".into(),
                org: false,
            },
        ),
    )
    .await;
    assert!(not_joined.unwrap_err().to_string().contains("isn't joined"));

    // A branch's channel is left like any topic's, and stays left whatever's checked out.
    alpha
        .request(Request::Leave {
            repo: alpha_clone.path().to_owned(),
            topic: "login".into(),
            org: false,
            session: None,
        })
        .await;
    alpha
        .eventually(
            |status| !status.channels.iter().any(|c| c == "#api/login"),
            "leaving #api/login",
        )
        .await;
}

#[tokio::test]
async fn a_post_too_long_for_a_line_arrives_whole() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, _beta_clone)] = two_machines(&ergo).await;
    let mut watching = beta.watch(None).await;

    // Several IRC lines' worth, with characters of more than one byte, sent as one IRCv3
    // multiline message rather than cut short.
    let text = "the plan: ".to_owned() + &"rebase, test, then land é ".repeat(60);
    assert!(text.len() > 3 * zirc_proto::MAX_BODY_BYTES);
    alpha
        .request(Request::Post {
            repo: alpha_clone.path().to_owned(),
            session: "cc-1".into(),
            text: text.clone(),
            to: Target::Repo,
            app: None,
            everyone: false,
            force: true,
            reply: None,
        })
        .await;
    let delivery = next_post(&mut watching).await;
    assert_eq!(delivery.from, "alpha");
    assert_eq!(delivery.message.event, Event::Text { body: text.clone() });
}

#[tokio::test]
async fn plain_posts_go_to_the_agents_thread_and_wake_only_whom_they_name() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    for session in ["cc-1", "cc-2", "cc-3"] {
        alpha
            .request(Request::RepoAdd {
                path: alpha_clone.path().to_owned(),
                session: Some(session.into()),
            })
            .await;
    }
    // Every machine watching the repository is in its agents channel, as in its channel.
    for machine in [&alpha, &beta] {
        machine
            .eventually(
                |status| status.channels.iter().any(|c| c == "#api/agents"),
                "joining #api/agents",
            )
            .await;
    }
    let mut cc1 = alpha.watch(Some("cc-1")).await;
    let mut cc2 = alpha.watch(Some("cc-2")).await;
    let post = async |machine: &Machine, clone: &TempDir, session: &str, text: &str| {
        let request = Request::Post {
            repo: clone.path().to_owned(),
            session: session.into(),
            text: text.into(),
            to: Target::Default,
            app: None,
            everyone: false,
            force: true,
            reply: None,
        };
        machine.request(request).await
    };
    // What a session is doing goes to the thread, and wakes no other session.
    let said = post(&beta, &beta_clone, "cc-9", "building the release, 2 of 4").await;
    assert_eq!(said, Response::channel("#api/agents"));
    for watching in [&mut cc1, &mut cc2] {
        assert_eq!(
            next_post_why(watching).await,
            ("building the release, 2 of 4".into(), None)
        );
    }
    // Addressed to one agent session known here, it's a note for that one alone, in no
    // room: only a post to a person known here goes to the repository's channel.
    let said = post(
        &alpha,
        &alpha_clone,
        "cc-3",
        "cc-1: can you rebase onto it?",
    )
    .await;
    assert_eq!(
        said,
        Response::Noted {
            to: "alpha/cc-1".into()
        }
    );
    let note = loop {
        if let ipc::Heard::Direct(direct) = heard(&mut cc1).await {
            break direct;
        }
    };
    assert!(note.note, "{note:?}");
    assert_eq!(
        (
            note.text.as_str(),
            note.session.as_deref(),
            note.from_session.as_deref(),
            note.why
        ),
        (
            "can you rebase onto it?",
            Some("cc-1"),
            Some("cc-3"),
            Some(Why::Direct)
        )
    );
    // Said in the thread naming a session, as a person might: that one alone wakes.
    let in_thread = Request::Post {
        repo: beta_clone.path().to_owned(),
        session: "cc-9".into(),
        text: "cc-2 has the green build".into(),
        to: Target::Topic {
            name: "agents".into(),
            org: false,
        },
        app: None,
        everyone: false,
        force: true,
        reply: None,
    };
    beta.request(in_thread).await;
    assert_eq!(
        next_post_why(&mut cc2).await,
        ("cc-2 has the green build".into(), Some(Why::Mention))
    );
    assert_eq!(
        next_post_why(&mut cc1).await,
        ("cc-2 has the green build".into(), None)
    );
}

#[tokio::test]
async fn a_post_goes_to_the_room_of_the_area_its_worktrees_changes_are_in() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    // The team gives the repository areas: each machine joins their rooms.
    let areas = "[rooms.areas]\nweb = [\"web/\"]\nplatform = [\"server/\", \"ops/\"]\n";
    for (machine, clone) in [(&alpha, &alpha_clone), (&beta, &beta_clone)] {
        std::fs::create_dir_all(clone.path().join(".zirc")).unwrap();
        std::fs::write(clone.path().join(".zirc/config.toml"), areas).unwrap();
        machine
            .eventually(
                |status| {
                    status.channels
                        == [
                            "#api",
                            "#api/agents",
                            "#api/gotchas",
                            "#api/landings",
                            "#api/platform",
                            "#api/web",
                            "#decisions",
                            "#general",
                            "#ops",
                        ]
                },
                "joining the areas' rooms",
            )
            .await;
    }
    // On beta, cc-w joined the web area as a topic; cc-x didn't.
    for session in ["cc-w", "cc-x"] {
        beta.request(Request::RepoAdd {
            path: beta_clone.path().to_owned(),
            session: Some(session.into()),
        })
        .await;
    }
    beta.request(Request::Join {
        repo: beta_clone.path().to_owned(),
        topic: "web".into(),
        org: false,
        session: Some("cc-w".into()),
    })
    .await;
    let mut cc_w = beta.watch(Some("cc-w")).await;
    let mut cc_x = beta.watch(Some("cc-x")).await;

    // Alpha's worktree changes mostly web's paths.
    std::fs::create_dir_all(alpha_clone.path().join("web")).unwrap();
    for file in ["web/login.ts", "web/form.ts"] {
        std::fs::write(alpha_clone.path().join(file), "export {};\n").unwrap();
    }
    std::fs::create_dir_all(alpha_clone.path().join("ops")).unwrap();
    std::fs::write(alpha_clone.path().join("ops/run.sh"), "true\n").unwrap();
    let post = |text: &str| plain_post(alpha_clone.path(), "cc-a", text, Target::Default);
    assert_eq!(
        alpha.request(post("the login form is wired up")).await,
        Response::channel("#api/web")
    );
    // News to whom joined the area, not to every session hearing its room; an agent's post
    // there waits for its next wake.
    assert_eq!(
        next_post_why(&mut cc_w).await,
        ("the login form is wired up".into(), None)
    );
    let said = next_post(&mut cc_x).await;
    assert_eq!((said.channel.as_str(), said.why), ("#api/web", None));
    // A status line still goes to the agents room, which every session hears.
    assert_eq!(
        alpha.request(post("taking: the login form")).await,
        Response::channel("#api/agents")
    );
    // Changes in two areas alike are in neither's: the agents room.
    std::fs::write(alpha_clone.path().join("ops/deploy.sh"), "true\n").unwrap();
    assert_eq!(
        alpha.request(post("both halves are in")).await,
        Response::channel("#api/agents")
    );
}

#[tokio::test]
async fn a_claim_in_the_agents_room_is_listed_as_the_repositorys() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, _beta_clone)] = two_machines(&ergo).await;
    let said = alpha
        .request(Request::Post {
            repo: alpha_clone.path().to_owned(),
            session: "cc-1".into(),
            text: "taking: the login page".into(),
            to: Target::Default,
            app: None,
            everyone: false,
            force: true,
            reply: None,
        })
        .await;
    assert_eq!(said, Response::channel("#api/agents"));
    // Another machine lists it under the repository, as zirc who shows it.
    until_who(
        &beta,
        |channels| {
            channels.iter().any(|c| {
                c.channel == "#api" && c.claims.iter().any(|claim| claim.session == "cc-1")
            })
        },
        "cc-1's claim under #api",
    )
    .await;
}

#[tokio::test]
async fn an_agents_post_in_a_topic_comes_with_the_next_wake() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    alpha
        .request(Request::RepoAdd {
            path: alpha_clone.path().to_owned(),
            session: Some("cc-1".into()),
        })
        .await;
    for (machine, clone, session) in [
        (&alpha, &alpha_clone, Some("cc-1")),
        (&beta, &beta_clone, None),
    ] {
        machine
            .request(Request::Join {
                repo: clone.path().to_owned(),
                topic: "deploys".into(),
                org: true,
                session: session.map(Into::into),
            })
            .await;
        machine
            .eventually(
                |status| status.channels.iter().any(|c| c == "#deploys"),
                "joining #deploys",
            )
            .await;
    }
    let mut cc1 = alpha.watch(Some("cc-1")).await;
    beta.request(to_deploys(&beta_clone, "deployer", "staging runs 1a2b3c"))
        .await;
    assert_eq!(
        next_post_why(&mut cc1).await,
        ("staging runs 1a2b3c".into(), None)
    );
    // A person's post there wakes cc-1, the agent's coming first.
    beta.request(to_deploys(&beta_clone, "beta", "staging looks wrong to me"))
        .await;
    assert_eq!(
        next_post_why(&mut cc1).await.0,
        "cc-1: meanwhile, for you and needing nothing yet:"
    );
    assert_eq!(
        next_post_why(&mut cc1).await,
        ("staging runs 1a2b3c".into(), Some(Why::Topic))
    );
    assert_eq!(
        next_post_why(&mut cc1).await,
        ("staging looks wrong to me".into(), Some(Why::Topic))
    );
}

#[tokio::test]
async fn a_topic_is_news_to_the_sessions_that_joined_it() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    // Two sessions in alpha's clone; only cc-1 joins the deploys topic.
    for session in ["cc-1", "cc-2"] {
        alpha
            .request(Request::RepoAdd {
                path: alpha_clone.path().to_owned(),
                session: Some(session.into()),
            })
            .await;
    }
    let join = async |machine: &Machine, clone: &TempDir, session: Option<&str>| {
        machine
            .request(Request::Join {
                repo: clone.path().to_owned(),
                topic: "deploys".into(),
                org: true,
                session: session.map(Into::into),
            })
            .await
    };
    join(&alpha, &alpha_clone, Some("cc-1")).await;
    join(&beta, &beta_clone, None).await;
    for machine in [&alpha, &beta] {
        machine
            .eventually(
                |status| status.channels.iter().any(|c| c == "#deploys"),
                "joining #deploys",
            )
            .await;
    }
    let mut cc1 = alpha.watch(Some("cc-1")).await;
    let mut cc2 = alpha.watch(Some("cc-2")).await;
    let deployer = |text: &str| to_deploys(&beta_clone, "deployer", text);

    // Another agent's post there waits for cc-1's next wake.
    beta.request(deployer("staging runs 1a2b3c")).await;
    assert_eq!(
        next_post_why(&mut cc1).await,
        ("staging runs 1a2b3c".into(), None),
        "cc-1 joined #deploys, but an agent's post waits"
    );
    assert_eq!(
        next_post_why(&mut cc2).await,
        ("staging runs 1a2b3c".into(), None),
        "cc-2 didn't, though its machine is in #deploys"
    );
    // `@everyone` reaches every session, joined or not: cc-1 hears the post that waited first.
    beta.request(deployer("@everyone: main is frozen")).await;
    for _ in 0..2 {
        let _ = next_post_why(&mut cc1).await;
    }
    for watching in [&mut cc1, &mut cc2] {
        assert_eq!(
            next_post_why(watching).await,
            ("@everyone: main is frozen".into(), Some(Why::Mention))
        );
    }
    // An agent writing the word in its text, as in a quote, broadcasts nothing.
    beta.request(deployer("the rule: write --everyone, not \"@everyone\""))
        .await;
    assert_eq!(
        next_post_why(&mut cc2).await,
        ("the rule: write --everyone, not \"@everyone\"".into(), None)
    );
    let _ = next_post_why(&mut cc1).await;
    // Once in a while: the same sender's next broadcast, soon after, wakes no one: cc-1
    // joined #deploys, where another agent's post waits for its next wake, and cc-2 didn't.
    beta.request(deployer("@everyone: still frozen")).await;
    assert_eq!(
        next_post_why(&mut cc1).await,
        ("@everyone: still frozen".into(), None)
    );
    assert_eq!(
        next_post_why(&mut cc2).await,
        ("@everyone: still frozen".into(), None)
    );
    // The limit is the machine's, whatever session its frame names, and a frame with no
    // session is a machine's all the same, not a person's own words.
    for session in ["deployer-2", ""] {
        let text = format!("@everyone: from {session:?}");
        beta.request(to_deploys(&beta_clone, session, &text)).await;
        assert_eq!(next_post_why(&mut cc2).await, (text, None));
        let _ = next_post_why(&mut cc1).await;
    }

    // The last session to leave a topic leaves its channel.
    let Response::Channel { channel, .. } = alpha
        .request(Request::Leave {
            repo: alpha_clone.path().to_owned(),
            topic: "deploys".into(),
            org: true,
            session: Some("cc-1".into()),
        })
        .await
    else {
        panic!("not a channel")
    };
    assert_eq!(channel, "#deploys");
    alpha
        .eventually(
            |status| !status.channels.iter().any(|c| c == "#deploys"),
            "leaving #deploys",
        )
        .await;
}

#[tokio::test]
async fn a_post_of_several_lines_goes_as_one_message_of_lines() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, _beta_clone)] = two_machines(&ergo).await;
    let (ada, mut ada_hears) = zirc_test_irc::person(&ergo, "ada").await;
    zirc_test_irc::joins(&ada, &mut ada_hears, "#api").await;
    let mut beta_hears = beta.watch(None).await;
    let body = "build failed:\n```\nerror[E0382]: borrow of moved value\n```";
    alpha
        .request(Request::Post {
            repo: alpha_clone.path().to_owned(),
            session: "deploy".into(),
            text: body.into(),
            to: Target::Repo,
            app: Some("CI".into()),
            everyone: false,
            force: true,
            reply: None,
        })
        .await;

    let said = next(&mut beta_hears).await;
    assert_eq!(said.message.event, Event::Text { body: body.into() });
    assert_eq!(said.message.app.as_deref(), Some("ci"));

    let privmsg = |line: &zirc_irc::Line| line.command == "PRIVMSG" && line.params[0] == "#api";
    let mut lines = Vec::new();
    while lines.len() < 4 {
        lines.push(zirc_test_irc::next_matching(&mut ada_hears, privmsg).await);
    }
    let texts: Vec<&str> = lines.iter().map(|line| line.params[1].as_str()).collect();
    assert!(texts[0].ends_with(":build failed:"), "{texts:?}");
    assert_eq!(
        texts[1..],
        ["```", "error[E0382]: borrow of moved value", "```\x01"]
    );
    assert!(lines.iter().all(|line| line.tag("batch").is_some()));
}
