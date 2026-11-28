//! zircd's budgets, as people and agents meet them: how soon a line said on the server
//! wakes the watch it concerns, and how much memory zircd holds once it has heard a lot.
//! Each fails with the number it measured and its budget, so whoever breaks one knows by
//! how much.

use std::time::{Duration, Instant};

use zirc_core::ipc::{Request, Why};
use zirc_proto::Event;
use zirc_test_ergo::require_ergo;

mod common;
use common::*;

/// How many wakes the latency is measured over.
const WAKES: usize = 20;

/// The 95th percentile of [`WAKES`] wakes, from a post's request on one machine to the
/// mention reaching the named session's watch on another (the user's choice, 28 Nov 2026):
/// that day's p95 was 86 ms on a Mac, in the build tests run, so this leaves room for a
/// busy CI agent. `zirc watch --once` adds its own 200 ms wait for the rest of a burst on
/// top (`ONCE_BURST` in the zirc crate's `watch.rs`).
const WAKE_P95: Duration = Duration::from_millis(250);

#[tokio::test]
async fn a_mention_wakes_the_session_it_names_within_its_budget() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, beta_clone)] = two_machines(&ergo).await;
    beta.request(Request::RepoAdd {
        path: beta_clone.path().to_owned(),
        session: Some("cc-b".into()),
    })
    .await;
    let mut took = Vec::with_capacity(WAKES);
    for n in 0..WAKES {
        // A session of its own each time: zircd pauses a session's wakes after 10 in 10
        // minutes (`zircd::daemon::breaker`).
        let session = format!("cc-a{n}");
        alpha
            .request(Request::RepoAdd {
                path: alpha_clone.path().to_owned(),
                session: Some(session.clone()),
            })
            .await;
        let mut woken = alpha.watch(Some(&session)).await;
        let text = format!("{session}: is the cache keyed by branch?");
        let asked = Instant::now();
        post(&beta, beta_clone.path(), "cc-b", &text).await;
        loop {
            let delivery = next_post(&mut woken).await;
            if delivery.why == Some(Why::Mention)
                && matches!(&delivery.message.event, Event::Text { body } if *body == text)
            {
                break;
            }
        }
        took.push(asked.elapsed());
    }
    took.sort();
    let p95 = took[took.len() * 95 / 100 - 1];
    let median = took[took.len() / 2];
    eprintln!("zircd wake: median {median:?}, p95 {p95:?} over {WAKES}");
    assert!(
        p95 <= WAKE_P95,
        "a mention woke its session in {p95:?} at the 95th percentile (median {median:?}), \
         over its budget of {WAKE_P95:?}: {took:?}"
    );
}

/// How many lines zircd hears in the soak, in batches of [`BATCH`].
const LINES: usize = 10_000;
const BATCH: usize = 200;

/// How many sessions watch on the machine hearing them.
const SESSIONS: usize = 50;

/// How much more memory zircd may hold after the soak than after its warm-up (the user's
/// choice, 28 Nov 2026): it grew 2 MB that day, so a leak of 1 KB a line fails it.
const RSS_GROWTH: u64 = 10 * 1024 * 1024;

/// How much CPU zircd may take while nothing happens, as a share of one core (the user's
/// choice, 28 Nov 2026): 0.00% that day.
const IDLE_CPU: f64 = 0.01;

/// How long the soak waits after its last line before measuring idle CPU.
const SETTLE: Duration = Duration::from_secs(5);

/// How long idle CPU is measured over.
const IDLE_FOR: Duration = Duration::from_secs(10);

#[tokio::test]
async fn zircd_hears_ten_thousand_lines_with_fifty_sessions_watching_in_bounded_memory() {
    let Some(mut soak) = Soak::start().await else {
        return;
    };
    soak.say(1_000).await;
    let before = settled_rss().await;
    let started = Instant::now();
    soak.say(LINES).await;
    let heard_in = started.elapsed();
    let after = settled_rss().await;
    let idle = idle_cpu().await;
    let grew = after.saturating_sub(before);
    let mb = |bytes: u64| bytes / 1024 / 1024;
    eprintln!(
        "zircd soak: {LINES} lines in {heard_in:?}, RSS {} -> {} MB, idle CPU {:.2}%",
        mb(before),
        mb(after),
        idle * 100.0
    );
    assert!(
        grew <= RSS_GROWTH,
        "zircd's memory grew {} MB over {LINES} lines with {SESSIONS} sessions watching \
         ({} -> {} MB), over its budget of {} MB",
        mb(grew),
        mb(before),
        mb(after),
        mb(RSS_GROWTH)
    );
    assert!(
        idle <= IDLE_CPU,
        "zircd took {:.2}% of a core while idle, over its budget of {:.0}%",
        idle * 100.0,
        IDLE_CPU * 100.0
    );
}

/// A machine with [`SESSIONS`] sessions watching, each read as `zirc watch` reads it, a
/// watch of everything it hears, and a person in `#agents` to say lines there.
struct Soak {
    _ergo: zirc_test_ergo::Ergo,
    _alpha: Machine,
    _clone: tempfile::TempDir,
    watches: Vec<tokio::task::JoinHandle<()>>,
    everything: zirc_core::ipc::Watching,
    chatty: zirc_irc::Client,
    said: usize,
}

impl Soak {
    /// The soak's machine, or `None` outside Bazel.
    async fn start() -> Option<Self> {
        // As fast as a connection can say them: it measures zircd, not Ergo's pacing.
        let ergo = zirc_test_ergo::Ergo::start_with(&[("ERGO__FAKELAG__ENABLED", "false")])?;
        let alpha = Machine::start(&ergo, "alpha").await;
        let clone = common::clone();
        let mut watches = Vec::new();
        for n in 0..SESSIONS {
            let session = format!("cc-{n}");
            alpha
                .request(Request::RepoAdd {
                    path: clone.path().to_owned(),
                    session: Some(session.clone()),
                })
                .await;
            let mut watching = alpha.watch(Some(&session)).await;
            // Read, so nothing waits on a full socket.
            watches.push(tokio::spawn(async move {
                while let Ok(Some(_)) = watching.next().await {}
            }));
        }
        alpha
            .eventually(|status| status.channels == JOINED, "joining")
            .await;
        let everything = alpha.watch(None).await;
        let (chatty, mut chatty_in) = zirc_test_irc::person(&ergo, "chatty").await;
        zirc_test_irc::joins(&chatty, &mut chatty_in, "#agents").await;
        tokio::spawn(async move { while chatty_in.recv().await.is_some() {} });
        Some(Self {
            _ergo: ergo,
            _alpha: alpha,
            _clone: clone,
            watches,
            everything,
            chatty,
            said: 0,
        })
    }

    /// Says `count` lines in `#agents` as agents' posts, [`BATCH`] at a time, each batch
    /// once zircd has heard the one before.
    async fn say(&mut self, count: usize) {
        for _ in 0..count / BATCH {
            for _ in 0..BATCH {
                self.said += 1;
                let message = zirc_proto::Message::new(
                    format!("cc-x{}", self.said % 20),
                    Event::Text {
                        body: Self::line(self.said),
                    },
                );
                self.chatty
                    .privmsg("#agents", &zirc_proto::encode(&message).unwrap())
                    .unwrap();
            }
            let last = Self::line(self.said);
            loop {
                if let Event::Text { body } = next(&mut self.everything).await.message.event
                    && body == last
                {
                    break;
                }
            }
        }
    }

    fn line(n: usize) -> String {
        format!("line {n}: the cache is keyed by branch, as before")
    }
}

impl Drop for Soak {
    fn drop(&mut self) {
        for watch in &self.watches {
            watch.abort();
        }
    }
}

/// [`rss`] a second after the last line, once what it set off has settled.
async fn settled_rss() -> u64 {
    tokio::time::sleep(Duration::from_secs(1)).await;
    rss()
}

/// The share of one core this process takes while nothing happens: over [`IDLE_FOR`], once
/// [`SETTLE`] has passed for what the soak set off to finish, such as state saved after it.
/// Each second's share is printed, so a failure shows a cost that stays from one that ebbs.
async fn idle_cpu() -> f64 {
    tokio::time::sleep(SETTLE).await;
    let threads = thread_ticks();
    let (start, from) = (cpu_time(), Instant::now());
    let mut seconds = Vec::new();
    let mut last = start;
    while from.elapsed() < IDLE_FOR {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let now = cpu_time();
        seconds.push(format!(
            "{:.1}%",
            now.saturating_sub(last).as_secs_f64() * 100.0
        ));
        last = now;
    }
    eprintln!("zircd idle, each second: {}", seconds.join(" "));
    let after = thread_ticks();
    let mut busiest: Vec<(String, u64)> = after
        .iter()
        .map(|(name, ticks)| {
            (
                name.clone(),
                ticks - threads.get(name).copied().unwrap_or(0),
            )
        })
        .filter(|(_, ticks)| *ticks > 0)
        .collect();
    busiest.sort_by_key(|(_, ticks)| std::cmp::Reverse(*ticks));
    eprintln!("zircd idle, ticks by thread: {busiest:?}");
    last.saturating_sub(start).as_secs_f64() / from.elapsed().as_secs_f64()
}

/// Each of this process's threads' CPU so far, in clock ticks, by its id and name.
#[cfg(target_os = "linux")]
fn thread_ticks() -> std::collections::HashMap<String, u64> {
    std::fs::read_dir("/proc/self/task")
        .unwrap()
        .filter_map(|task| {
            let stat = std::fs::read_to_string(task.ok()?.path().join("stat")).ok()?;
            let name = stat[stat.find('(')? + 1..stat.rfind(')')?].to_owned();
            let id = stat.split(' ').next()?.to_owned();
            let after = &stat[stat.rfind(')')? + 2..];
            let ticks = after
                .split(' ')
                .skip(11)
                .take(2)
                .map(|field| field.parse::<u64>().unwrap_or(0))
                .sum();
            Some((format!("{id} {name}"), ticks))
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn thread_ticks() -> std::collections::HashMap<String, u64> {
    std::collections::HashMap::new()
}

/// This process's resident memory, in bytes: zircd's, as the daemon runs in the test.
#[cfg(target_os = "linux")]
fn rss() -> u64 {
    // Its second field is the resident set, in pages: 4 KiB on the machines CI runs on.
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap();
    statm.split(' ').nth(1).unwrap().parse::<u64>().unwrap() * 4096
}

/// The CPU this process has taken, in user and system time.
#[cfg(target_os = "linux")]
fn cpu_time() -> Duration {
    // After the command's name in brackets, utime and stime are the 12th and 13th fields,
    // in clock ticks of 1/100 s.
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
    let after = &stat[stat.rfind(')').unwrap() + 2..];
    let ticks: u64 = after
        .split(' ')
        .skip(11)
        .take(2)
        .map(|field| field.parse::<u64>().unwrap())
        .sum();
    Duration::from_millis(ticks * 10)
}

/// This process's resident memory, in bytes: zircd's, as the daemon runs in the test.
#[cfg(not(target_os = "linux"))]
fn rss() -> u64 {
    ps("rss").parse::<u64>().expect("ps's rss") * 1024
}

/// The CPU this process has taken, in user and system time.
#[cfg(not(target_os = "linux"))]
fn cpu_time() -> Duration {
    // macOS's ps prints it as [hh:]mm:ss.cc.
    let seconds = ps("time").split(':').fold(0.0, |total, part| {
        total * 60.0 + part.parse::<f64>().expect("ps's time")
    });
    Duration::from_secs_f64(seconds)
}

#[cfg(not(target_os = "linux"))]
fn ps(field: &str) -> String {
    let out = std::process::Command::new("ps")
        .args([
            "-o",
            &format!("{field}="),
            "-p",
            &std::process::id().to_string(),
        ])
        .output()
        .expect("running ps");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
