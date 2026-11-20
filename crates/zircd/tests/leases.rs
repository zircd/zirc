//! Leases between two machines on one Ergo: one session holds a lease at a time, the other
//! waits its turn, both machines agree who's who, and the one waiting hears when it's theirs.

use std::time::Duration;

use tempfile::TempDir;
use zirc_core::ipc::{Heard, Request, Response, Watching};
use zirc_core::leases::Lease;
use zirc_proto::Event;
use zirc_test_ergo::{Ergo, require_ergo};

mod common;
use common::{Machine, WAIT};

/// A machine that watches a clone, which puts it in `#general`.
struct Member {
    machine: Machine,
    clone: TempDir,
}

impl std::ops::Deref for Member {
    type Target = Machine;
    fn deref(&self) -> &Machine {
        &self.machine
    }
}

impl Member {
    async fn start(ergo: &Ergo, nick: &str) -> Self {
        let machine = Machine::connect(ergo, nick).await;
        let clone = zirc_test_ergo::git::clone("git@github.com:acme/app.git");
        machine.add_repo(None, clone.path()).await.unwrap();
        machine
            .eventually(
                |status| status.channels.iter().any(|it| it == "#general"),
                "joining #general",
            )
            .await;
        Self { machine, clone }
    }

    async fn lease(&self, name: &str, session: &str, seconds: u64) {
        self.request(Request::Lease {
            name: name.into(),
            session: session.into(),
            seconds: Some(seconds),
            pid: None,
            wait: false,
        })
        .await;
    }

    /// Asks for `name` as `zirc lease --wait` does, from this test's own process.
    async fn lease_waiting(&self, name: &str, session: &str, seconds: u64) {
        self.request(Request::Lease {
            name: name.into(),
            session: session.into(),
            seconds: Some(seconds),
            pid: Some(std::process::id()),
            wait: true,
        })
        .await;
    }

    /// The lease `name` as this machine sees it.
    async fn seen(&self, name: &str) -> Option<Lease> {
        let Ok(Response::Leases { leases }) = self.try_request(Request::Leases).await else {
            return None;
        };
        leases.into_iter().find(|lease| lease.name == name)
    }

    /// Waits until this machine sees the lease `name` as `holds` says.
    async fn sees(&self, name: &str, what: &str, holds: impl Fn(Option<&Lease>) -> bool) {
        tokio::time::timeout(WAIT, async {
            while !holds(self.seen(name).await.as_ref()) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }
}

/// Whose `lease` is, as `nick/session`, with who's waiting.
fn whose(lease: Option<&Lease>) -> (Option<String>, Vec<String>) {
    let who = |it: &zirc_core::leases::Holder| format!("{}/{}", it.nick, it.session);
    lease.map_or((None, Vec::new()), |lease| {
        (
            lease.holder.as_ref().map(who),
            lease.waiting.iter().map(who).collect(),
        )
    })
}

/// The next lease line `watching` hears.
async fn next_lease_line(watching: &mut Watching) -> String {
    tokio::time::timeout(WAIT, async {
        loop {
            if let Heard::Message(delivery) = watching.next().await.unwrap().unwrap()
                && let Event::Custom { kind, text, .. } = &delivery.message.event
                && kind == zirc_core::leases::LEASE_KIND
                && delivery.why.is_some()
            {
                return text.clone();
            }
        }
    })
    .await
    .expect("no lease line heard")
}

#[tokio::test]
async fn one_session_holds_a_lease_at_a_time_and_the_next_hears_its_turn() {
    let ergo = require_ergo!();
    let alpha = Member::start(&ergo, "alpha").await;
    let beta = Member::start(&ergo, "beta").await;
    let mut beta_hears = beta.watch(Some("cc-b")).await;
    let mut alpha_hears = alpha.watch(Some("cc-a")).await;

    alpha.lease("emulator", "cc-a", 600).await;
    let held_by_alpha = |lease: Option<&Lease>| whose(lease).0.as_deref() == Some("alpha/cc-a");
    alpha
        .sees("emulator", "alpha holding it", held_by_alpha)
        .await;
    beta.sees("emulator", "beta hearing alpha holds it", held_by_alpha)
        .await;

    // Beta waits its turn, and both machines agree on the queue.
    beta.lease("emulator", "cc-b", 600).await;
    let queued = |lease: Option<&Lease>| {
        whose(lease) == (Some("alpha/cc-a".into()), vec!["beta/cc-b".into()])
    };
    beta.sees("emulator", "beta queued behind alpha", queued)
        .await;
    alpha
        .sees("emulator", "alpha seeing beta queued", queued)
        .await;
    let lease = beta.seen("emulator").await.unwrap();
    assert!(!lease.holder.unwrap().mine && lease.waiting[0].mine);

    // Nobody gives back another session's lease.
    let refused = beta
        .try_request(Request::LeaseRelease {
            name: "emulator".into(),
            session: "cc-a".into(),
        })
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("neither holds"), "{refused}");

    // Alpha gives it back: beta's turn, and its watch hears so.
    alpha
        .try_request(Request::LeaseRelease {
            name: "emulator".into(),
            session: "cc-a".into(),
        })
        .await
        .unwrap();
    let line = next_lease_line(&mut beta_hears).await;
    assert!(line.starts_with("cc-b: lease emulator is yours"), "{line}");
    // Alpha's was free when it asked: `zirc lease` said it was its, so its watch doesn't.
    // Beta naming cc-a comes after anything alpha's watch would have been told of it.
    beta.request(common::plain_post(
        beta.clone.path(),
        "cc-b",
        "cc-a: marker",
        zirc_core::ipc::Target::Agents,
    ))
    .await;
    let told: Vec<String> = common::until_marker(&mut alpha_hears, |heard| {
        common::says(heard, "cc-a: marker")
    })
    .await
    .iter()
    .filter_map(|heard| match heard {
        Heard::Message(delivery) if delivery.why.is_some() => match &delivery.message.event {
            Event::Custom { kind, text, .. } if kind == zirc_core::leases::LEASE_KIND => {
                Some(text.clone())
            }
            _ => None,
        },
        _ => None,
    })
    .collect();
    assert!(told.is_empty(), "{told:?}");
    let held_by_beta = |lease: Option<&Lease>| whose(lease).0.as_deref() == Some("beta/cc-b");
    alpha
        .sees("emulator", "alpha seeing beta hold it", held_by_beta)
        .await;

    // A lease ends by itself.
    alpha.lease("simulator", "cc-a", 2).await;
    alpha
        .sees("simulator", "alpha holding the simulator", |lease| {
            whose(lease).0.is_some()
        })
        .await;
    beta.sees("simulator", "the simulator's lease ending", |lease| {
        lease.is_none()
    })
    .await;
}

#[tokio::test]
async fn three_sessions_hold_a_build_at_once_and_a_fourth_waits() {
    let ergo = require_ergo!();
    let alpha = Member::start(&ergo, "alpha").await;
    let beta = Member::start(&ergo, "beta").await;
    alpha.lease("build", "cc-a", 600).await;
    beta.lease("build", "cc-b", 600).await;
    alpha.lease("build", "cc-c", 600).await;
    beta.lease("build", "cc-d", 600).await;
    let three = |lease: Option<&Lease>| {
        lease.is_some_and(|lease| {
            let sessions = |list: &[zirc_core::leases::Holder]| {
                list.iter().map(|it| it.session.clone()).collect::<Vec<_>>()
            };
            sessions(&lease.holders) == ["cc-a", "cc-b", "cc-c"]
                && sessions(&lease.waiting) == ["cc-d"]
        })
    };
    alpha
        .sees("build", "three holders and one waiting", three)
        .await;
    beta.sees("build", "beta agreeing", three).await;
}

#[tokio::test]
async fn lease_events_stay_out_of_history_and_reach_a_machine_that_joins_later() {
    let ergo = require_ergo!();
    let (ada, mut heard) = zirc_test_irc::person(&ergo, "ada").await;
    ada.join("#general").unwrap();
    let alpha = Member::start(&ergo, "alpha").await;
    alpha.lease("emulator", "cc-1", 600).await;
    // A person in the channel gets it as a TAGMSG, which no history keeps.
    let line = zirc_test_irc::next_matching(&mut heard, |line| {
        line.command == "TAGMSG" && line.tag(zirc_proto::EVENT_TAG).is_some()
    })
    .await;
    let message = zirc_proto::from_payload(line.tag(zirc_proto::EVENT_TAG).unwrap()).unwrap();
    assert!(
        matches!(&message.event, Event::Custom { kind, .. } if kind == zirc_core::leases::LEASE_KIND),
        "{message:?}"
    );
    alpha
        .sees("emulator", "alpha holding it", |lease| {
            whose(lease).0.as_deref() == Some("alpha/cc-1")
        })
        .await;
    // A machine that joins afterwards hears it said again, and agrees who holds it.
    let beta = Member::start(&ergo, "beta").await;
    beta.sees("emulator", "beta seeing alpha hold it", |lease| {
        whose(lease).0.as_deref() == Some("alpha/cc-1")
    })
    .await;
}

#[tokio::test]
async fn a_session_waiting_with_wait_gets_its_turn_without_its_watch_waking() {
    let ergo = require_ergo!();
    let alpha = Member::start(&ergo, "alpha").await;
    let beta = Member::start(&ergo, "beta").await;
    let mut beta_hears = beta.watch(Some("cc-b")).await;
    alpha.lease("emulator", "cc-a", 600).await;
    let held_by =
        |who: &'static str| move |lease: Option<&Lease>| whose(lease).0.as_deref() == Some(who);
    beta.sees(
        "emulator",
        "beta hearing alpha holds it",
        held_by("alpha/cc-a"),
    )
    .await;

    // `zirc lease --wait` is told by asking, so the watch hears nothing of it.
    beta.lease_waiting("emulator", "cc-b", 600).await;
    alpha
        .request(Request::LeaseRelease {
            name: "emulator".into(),
            session: "cc-a".into(),
        })
        .await;
    beta.sees("emulator", "beta holding it", held_by("beta/cc-b"))
        .await;
    let woke = tokio::time::timeout(Duration::from_secs(3), next_lease_line(&mut beta_hears)).await;
    assert!(woke.is_err(), "the watch woke: {woke:?}");
}
