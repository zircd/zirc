//! How long the work an app repeats on every frame or every message takes with a big
//! workspace, printed with `--nocapture`, and the speed budget it must keep to in a busy
//! organisation ([`a_busy_organisation_refreshes_within_the_budget`]). The limits are loose:
//! they catch a quadratic return, not noise.

use std::time::Instant;

use chrono::FixedOffset;

use super::*;
use crate::chat::ListOptions;

fn big(channels: usize, per_channel: usize) -> Chat {
    let mut chat = bare();
    chat.connected(link(true));
    for c in 0..channels {
        feed(&mut chat, &[&format!(":me!u@h JOIN #c{c}")]);
        let mut batch = vec![format!(":s BATCH +h{c} chathistory #c{c}")];
        for m in 0..per_channel {
            batch.push(format!(
                "@batch=h{c};time=2026-10-30T11:{:02}:00.000Z;msgid=m{c}-{m} :bob!u@h PRIVMSG #c{c} :message {m} with some words",
                m % 60
            ));
        }
        batch.push(format!(":s BATCH -h{c}"));
        feed_all(&mut chat, &batch);
    }
    chat
}

fn time<T>(what: &str, runs: u32, mut f: impl FnMut() -> T) -> f64 {
    let start = Instant::now();
    for _ in 0..runs {
        std::hint::black_box(f());
    }
    let each = start.elapsed().as_secs_f64() * 1000.0 / f64::from(runs);
    println!("PERF {what}: {each:.3} ms");
    each
}

#[test]
fn a_workspace_of_hundreds_of_channels_stays_quick() {
    let mut chat = big(300, 50);
    time("sidebar_list, 300 channels", 20, || {
        chat.sidebar_list(&ListOptions::default())
    });
    time("lines, one channel of 50", 20, || {
        chat.lines("#c7", chrono::FixedOffset::east_opt(0).unwrap(), false)
    });
    let mut n = 0;
    time("receive one message, 300 channels", 200, || {
        n += 1;
        feed(
            &mut chat,
            &[&format!(
                "@time=2026-10-30T12:00:00.000Z;msgid=n{n} :bob!u@h PRIVMSG #c{} :live {n}",
                n % 300
            )],
        )
    });
}

#[test]
fn a_channel_with_long_history_stays_quick() {
    let mut chat = bare();
    chat.connected(link(true));
    feed(&mut chat, &[":me!u@h JOIN #api"]);
    // Catch-up pages of 100, oldest last, as scrolling back or a reconnect brings them.
    let started = Instant::now();
    for p in 0..50u32 {
        let lines = page(&format!("p{p}"), (p * 100 + 1)..=(p * 100 + 100));
        feed_all(&mut chat, &lines);
    }
    println!(
        "PERF 50 history pages of 100: {:.3} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    let offset = chrono::FixedOffset::east_opt(0).unwrap();
    time("lines, 5000 messages", 5, || {
        chat.lines("#api", offset, false)
    });
    time("lines people_only, 5000", 5, || {
        chat.lines("#api", offset, true)
    });
    let mut n = 0;
    time("receive one message, 5000 history", 200, || {
        n += 1;
        feed(
            &mut chat,
            &[&format!(
                "@time=2026-10-30T12:00:00.000Z;msgid=n{n} :bob!u@h PRIVMSG #api :live {n}"
            )],
        )
    });
    time("Entry::rich over the 2000 kept", 5, || {
        chat.conversation("#api")
            .unwrap()
            .entries
            .iter()
            .map(|e| e.rich().len())
            .sum::<usize>()
    });
    time("search 5000", 5, || chat.search("#api", "4999"));
}

#[test]
fn a_full_conversation_drops_its_oldest_in_batches_not_one_by_one() {
    let mut chat = bare();
    chat.connected(link(true));
    feed(&mut chat, &[":me!u@h JOIN #api"]);
    let max = chat.options.max_entries;
    let mut changes = 0;
    let mut dropped = 0;
    for n in 0..max * 2 {
        feed(
            &mut chat,
            &[&format!(
                "@time=2026-10-30T12:00:00.000Z;msgid=n{n} :bob!u@h PRIVMSG #api :live {n}"
            )],
        );
        let api = chat.conversation("#api").unwrap();
        assert!(
            api.entries.len() <= max + max / 8,
            "{} kept",
            api.entries.len()
        );
        if api.dropped != dropped {
            dropped = api.dropped;
            changes += 1;
        }
    }
    println!("PERF {changes} drops over {} messages", max * 2);
    // A view starts over at each; one per message was the cost of a busy channel.
    assert!(changes <= 8, "{changes} drops");
}

/// The fastest of `runs` runs of `f`, in milliseconds, printed as `what`: the least the
/// machine let it take, which another test running beside it moves least.
fn fastest<T>(what: &str, runs: u32, mut f: impl FnMut() -> T) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..runs {
        let start = Instant::now();
        std::hint::black_box(f());
        best = best.min(start.elapsed().as_secs_f64() * 1000.0);
    }
    println!("PERF {what}: {best:.3} ms");
    best
}

/// What zirc-web's view asks of the core each time the page redraws: for every line that
/// comes in and every key typed, as the typing notice redraws (`crates/zirc-web/src/view.rs`).
fn web(chat: &Chat, zone: FixedOffset) -> usize {
    use super::busy::OPEN;
    let wall = |at: DateTime<Utc>| at.with_timezone(&zone).naive_local();
    let rows = chat
        .conversations()
        .iter()
        .map(|c| {
            (
                chat.title(&c.name),
                chat.short_title(&c.name),
                chat.channel_about(&c.name),
            )
        })
        .collect::<Vec<_>>()
        .len();
    rows + chat.lines(OPEN, zone, false).len()
        + chat
            .sidebar_list(&ListOptions {
                tab: Some(chat.tab()),
                ..ListOptions::default()
            })
            .len()
        + chat.tabs().len()
        + chat.threads(OPEN).len()
        + chat.member_count(OPEN)
        + usize::from(chat.intro(OPEN).is_some())
        + usize::from(chat.read_only_note(OPEN).is_some())
        + usize::from(chat.unseen(wall).is_some())
        + chat.org_header().name.len()
        + chat.profile(chat.me()).card(noon(), wall).title.len()
        + usize::from(chat.header_line(OPEN).is_some())
        + chat.show_choices(OPEN).len()
        + chat.member_rows(OPEN).len()
        + chat.app_members(OPEN).len()
        + chat.inbox_button().label.len()
}

/// What Android's refresh asks of the mobile core, the most any phone asks: every
/// conversation's row, the sidebar three times, both halves and each, and the open
/// conversation's lines, people and Activity (`AppModel.refresh`, `view::channels`).
fn phone(chat: &Chat, zone: FixedOffset) -> usize {
    use super::busy::OPEN;
    let wall = |at: DateTime<Utc>| at.with_timezone(&zone).naive_local();
    let rows = chat
        .channel_rows()
        .iter()
        .map(|row| {
            (
                chat.badge(&row.name),
                chat.short_title(&row.name),
                chat.subtitle(&row.name),
                chat.channel_about(&row.name),
            )
        })
        .collect::<Vec<_>>()
        .len()
        + chat.channel_groups().len()
        + chat.direct_rows().len();
    let sidebar = [
        None,
        Some(SidebarSection::Channels),
        Some(SidebarSection::Direct),
    ]
    .map(|tab| {
        chat.sidebar_list(&ListOptions {
            tab,
            ..ListOptions::default()
        })
        .len()
    })
    .iter()
    .sum::<usize>();
    rows + sidebar
        + chat.tabs().len()
        + chat.lines(OPEN, zone, false).len()
        + chat.activity().len()
        + usize::from(chat.badges().total > 0)
        + chat.threads(OPEN).len()
        + usize::from(chat.header_line(OPEN).is_some())
        + chat.show_choices(OPEN).len()
        + 2 * chat.member_rows(OPEN).len()
        + chat.app_members(OPEN).len()
        + usize::from(chat.unseen(wall).is_some())
        + chat.org_header().name.len()
}

/// What the desktop asks when the session changes (`changed`) and draws (`render`): the
/// open conversation's lines, the sidebar's half shown, the header and its people.
fn desktop(chat: &Chat, zone: FixedOffset) -> usize {
    use super::busy::OPEN;
    let wall = |at: DateTime<Utc>| at.with_timezone(&zone).naive_local();
    chat.lines(OPEN, zone, false).len()
        + chat
            .sidebar_list(&ListOptions {
                tab: Some(chat.tab()),
                ..ListOptions::default()
            })
            .len()
        + chat.tabs().len()
        + 2 * usize::from(chat.header_line(OPEN).is_some())
        + chat.member_rows(OPEN).len()
        + usize::from(chat.unseen(wall).is_some())
        + chat.inbox_button().label.len()
}

/// What the TUI asks each time it draws: the open conversation's items, the sidebar's half
/// shown and both halves, to go to the next by key (`App::sidebar`, `App::listed`).
fn tui(chat: &Chat, zone: FixedOffset) -> usize {
    use super::busy::OPEN;
    let conversation = chat.conversation(OPEN).expect("the open channel");
    chat.items(conversation, |time| time.with_timezone(&zone).date_naive())
        .len()
        + [Some(chat.tab()), None]
            .map(|tab| {
                chat.sidebar_list(&ListOptions {
                    tab,
                    ..ListOptions::default()
                })
                .len()
            })
            .iter()
            .sum::<usize>()
        + chat.tabs().len()
}

/// The speed budget, which the tests below keep to: what each app asks of the core on every
/// refresh, with a busy organisation open at its busiest channel ([`super::busy`]), takes
/// no longer than its budget here, in this test's build (the landing's, unoptimised), the
/// fastest of a few runs. Each budget is about four times what it took on an M-series Mac
/// when it was set (19 Nov 2026), and 1 ms at least, so a slower machine running other
/// tests beside it passes, while work that grows with the square of the conversations or
/// the lines, as the agent feed's did and a channel's header did until then (16 times this
/// budget's), fails. Optimised, as the apps run it, each takes a sixth to a tenth of it;
/// the chat plan's speed budget has those numbers.
#[derive(Default)]
struct Budget {
    /// What took longer than its budget, as the failure says it.
    over: Vec<String>,
}

impl Budget {
    /// The busy organisation, as long as it took to make.
    fn busy() -> Chat {
        let started = Instant::now();
        let chat = super::busy::busy();
        println!(
            "PERF build the busy organisation: {:.3} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
        chat
    }

    /// `f`, `what` it does, may take `budget` milliseconds.
    fn keeps<T>(&mut self, what: &str, budget: f64, f: impl FnMut() -> T) {
        let took = fastest(what, 5, f);
        if took > budget {
            self.over
                .push(format!("{what}: {took:.1} ms, over its {budget} ms"));
        }
    }

    fn kept(self) {
        assert!(
            self.over.is_empty(),
            "over the speed budget: {:#?}",
            self.over
        );
    }
}

#[test]
fn each_app_refreshes_a_busy_organisation_within_the_budget() {
    let chat = Budget::busy();
    let zone = FixedOffset::east_opt(0).unwrap();
    let mut budget = Budget::default();
    budget.keeps("web's refresh", 340.0, || web(&chat, zone));
    budget.keeps("a phone's refresh", 480.0, || phone(&chat, zone));
    budget.keeps("the desktop's refresh", 260.0, || desktop(&chat, zone));
    budget.keeps("the TUI's draw", 120.0, || tui(&chat, zone));
    budget.kept();
}

#[test]
fn what_a_refresh_asks_of_a_busy_organisation_is_within_the_budget() {
    use super::busy::{self, OPEN};
    let mut chat = Budget::busy();
    let zone = FixedOffset::east_opt(0).unwrap();
    let wall = |at: DateTime<Utc>| at.naive_utc();
    let mut budget = Budget::default();
    // The most costly first.
    budget.keeps("lines", 180.0, || chat.lines(OPEN, zone, false));
    budget.keeps("agent_lines", 10.0, || {
        chat.agent_lines("p03-m3/cc-3", zone)
    });
    budget.keeps("sidebar_list", 52.0, || {
        chat.sidebar_list(&ListOptions::default())
    });
    budget.keeps("every conversation's channel_about", 50.0, || {
        chat.conversations()
            .iter()
            .map(|c| chat.channel_about(&c.name))
            .collect::<Vec<_>>()
    });
    budget.keeps("member_rows", 16.0, || chat.member_rows(OPEN));
    budget.keeps("app_members", 16.0, || chat.app_members(OPEN));
    budget.keeps("activity", 20.0, || chat.activity());
    budget.keeps("badges", 9.0, || chat.badges());
    budget.keeps("inbox_button", 8.0, || chat.inbox_button());
    budget.keeps("org_header", 18.0, || chat.org_header());
    budget.keeps("direct_rows", 26.0, || chat.direct_rows());
    budget.keeps("header_line", 1.0, || chat.header_line(OPEN));
    budget.keeps("unseen", 1.0, || chat.unseen(wall));
    // Asked as the person types into search and Jump to.
    budget.keeps("search here", 190.0, || chat.search(OPEN, "flaky"));
    budget.keeps("switcher", 6.0, || chat.switcher("c1"));
    let mut n = 0;
    budget.keeps("receive one line", 1.0, || {
        n += 1;
        chat.receive(busy::live(n))
    });
    budget.kept();
}
