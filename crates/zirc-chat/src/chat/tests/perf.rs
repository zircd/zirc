//! How long the work an app repeats on every frame or every message takes with a big
//! workspace, printed with `--nocapture`, and the speed budget it keeps to in a busy
//! organisation: it grows no faster than the organisation ([`Budget`]). The limits are
//! loose: they catch a quadratic return, not noise.

use std::time::Instant;

use chrono::FixedOffset;

use super::busy::Size;
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

/// What zirc-web's view asks of the core each time the page redraws: for every line that
/// comes in and every key typed, as the typing notice redraws (`crates/zirc-web/src/view.rs`). A copy, as Rust can't
/// time the app's own here: `view` names this, so what it adds is added here.
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
            })
            .len()
        + chat.tabs().len()
        + usize::from(chat.waiting_card(Some(chat.tab())).is_some())
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
/// conversation's lines, people and Activity (`AppModel.refresh`, `view::channels`). A copy, as above: `AppModel.refresh`
/// and Swift's `Snapshot.read` name this.
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
        + chat.direct_rows().len()
        // The open conversation's row (`view::open_row`), and its faces: its own row alone.
        + chat.shown_row().map_or(0, |row| {
            row.title.len() + chat.direct_row(row.name).avatars.len()
        });
    let sidebar = [
        None,
        Some(SidebarSection::Channels),
        Some(SidebarSection::Direct),
    ]
    .map(|tab| chat.sidebar_list(&ListOptions { tab }).len())
    .iter()
    .sum::<usize>();
    rows + sidebar
        + chat.tabs().len()
        + usize::from(chat.waiting_card(Some(chat.tab())).is_some())
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
/// open conversation's lines, the sidebar's half shown, the header and its people. A copy, as above: `changed` and `render` name this.
fn desktop(chat: &Chat, zone: FixedOffset) -> usize {
    use super::busy::OPEN;
    let wall = |at: DateTime<Utc>| at.with_timezone(&zone).naive_local();
    chat.lines(OPEN, zone, false).len()
        + chat
            .sidebar_list(&ListOptions {
                tab: Some(chat.tab()),
            })
            .len()
        + chat.tabs().len()
        + usize::from(chat.waiting_card(Some(chat.tab())).is_some())
        + 2 * usize::from(chat.header_line(OPEN).is_some())
        + chat.member_rows(OPEN).len()
        + usize::from(chat.unseen(wall).is_some())
        + chat.inbox_button().label.len()
}

/// What the TUI asks each time it draws: the open conversation's items, the sidebar's half
/// shown and both halves, to go to the next by key (`App::sidebar`, `App::listed`). A copy, as above: `ui::render`
/// names this.
fn tui(chat: &Chat, zone: FixedOffset) -> usize {
    use super::busy::OPEN;
    let conversation = chat.conversation(OPEN).expect("the open channel");
    chat.items(conversation, |time| time.with_timezone(&zone).date_naive())
        .len()
        + [Some(chat.tab()), None]
            .map(|tab| chat.sidebar_list(&ListOptions { tab }).len())
            .iter()
            .sum::<usize>()
        + chat.tabs().len()
        + usize::from(chat.waiting_card(Some(chat.tab())).is_some())
}

/// How many times longer what an app asks may take in the busy organisation than in a
/// quarter of it ([`Size`]), where there's four times as much of everything. Work that grows
/// with the organisation took 4 to 8 times as long when this was set (19 Nov 2026), more
/// than 4 as the bigger one fits the processor's caches less well; work that grows with its
/// square takes 16, as a channel's header line and a channel's app members did until then,
/// and the agent feed's refresh before them.
const GROWTH: f64 = 12.0;

/// What takes less than this in the busy organisation, in milliseconds, is too quick for
/// its growth to be timed, nor costs a refresh anything.
const TOO_QUICK: f64 = 1.0;

/// The speed budget, which the tests below keep to: what each app asks of the core on
/// every refresh may grow no faster than the organisation ([`GROWTH`]). How long a call
/// takes depends on the machine and what else runs on it (a loaded CI agent ran these
/// 4 to 19 times slower than an M-series Mac), but how much longer it takes in an
/// organisation four times the size doesn't: each is timed on both, in turn, the fastest
/// of a few runs. The chat plan's speed budget has what each took.
struct Budget {
    quarter: Chat,
    busy: Chat,
    /// What grew too fast, as the failure says it.
    over: Vec<String>,
}

impl Budget {
    /// Both organisations, as long as each took to make.
    fn new() -> Self {
        let made = |size: Size| {
            let started = Instant::now();
            let chat = super::busy::organisation(size);
            println!(
                "PERF make {size:?}: {:.3} ms",
                started.elapsed().as_secs_f64() * 1000.0
            );
            chat
        };
        Self {
            quarter: made(Size::QUARTER),
            busy: made(Size::BUSY),
            over: Vec::new(),
        }
    }

    /// `f`, `what` it asks, grows no faster than the organisation.
    fn keeps<T>(&mut self, what: &str, mut f: impl FnMut(&mut Chat) -> T) {
        // Measured again when over: a spike of load on a busy machine (a load average of
        // 60 once made a call measure 15 times) seldom comes twice, work that grows too
        // fast always does.
        let mut growth = f64::MAX;
        for _ in 0..2 {
            let (mut quarter, mut busy) = (f64::MAX, f64::MAX);
            for _ in 0..7 {
                quarter = quarter.min(took(&mut f, &mut self.quarter));
                busy = busy.min(took(&mut f, &mut self.busy));
            }
            growth = busy / quarter.max(f64::MIN_POSITIVE);
            println!("PERF {what}: {busy:.3} ms, a quarter of it {quarter:.3} ms, {growth:.1}×");
            if growth <= GROWTH || busy < TOO_QUICK {
                return;
            }
        }
        self.over.push(format!(
            "{what}: {growth:.1} times as long as in a quarter of the organisation, twice"
        ));
    }

    fn kept(self) {
        assert!(
            self.over.is_empty(),
            "grows faster than the organisation, more than {GROWTH} times a quarter of it: {:#?}",
            self.over
        );
    }
}

/// How long `f` takes on `chat`, in milliseconds.
fn took<T>(f: &mut impl FnMut(&mut Chat) -> T, chat: &mut Chat) -> f64 {
    let start = Instant::now();
    std::hint::black_box(f(chat));
    start.elapsed().as_secs_f64() * 1000.0
}

#[test]
fn each_apps_refresh_grows_no_faster_than_the_organisation() {
    let zone = FixedOffset::east_opt(0).unwrap();
    let mut budget = Budget::new();
    budget.keeps("web's refresh", |chat| web(chat, zone));
    budget.keeps("a phone's refresh", |chat| phone(chat, zone));
    budget.keeps("the desktop's refresh", |chat| desktop(chat, zone));
    budget.keeps("the TUI's draw", |chat| tui(chat, zone));
    budget.kept();
}

#[test]
fn what_a_refresh_asks_grows_no_faster_than_the_organisation() {
    use super::busy::{self, AGENT, OPEN};
    let zone = FixedOffset::east_opt(0).unwrap();
    let wall = |at: DateTime<Utc>| at.naive_utc();
    let mut budget = Budget::new();
    budget.keeps("lines", |chat| chat.lines(OPEN, zone, false));
    budget.keeps("agent_lines", |chat| chat.agent_lines(AGENT, zone));
    budget.keeps("sidebar_list", |chat| {
        chat.sidebar_list(&ListOptions::default())
    });
    budget.keeps("every conversation's channel_about", |chat| {
        chat.conversations()
            .iter()
            .map(|c| chat.channel_about(&c.name))
            .collect::<Vec<_>>()
    });
    budget.keeps("member_rows", |chat| chat.member_rows(OPEN));
    budget.keeps("app_members", |chat| chat.app_members(OPEN));
    budget.keeps("activity", |chat| chat.activity());
    budget.keeps("badges", |chat| chat.badges());
    budget.keeps("inbox_button", |chat| chat.inbox_button());
    budget.keeps("org_header", |chat| chat.org_header());
    budget.keeps("direct_rows", |chat| chat.direct_rows());
    budget.keeps("header_line", |chat| chat.header_line(OPEN));
    budget.keeps("unseen", |chat| chat.unseen(wall));
    // Asked as the person types into search and Jump to.
    budget.keeps("search here", |chat| chat.search(OPEN, "flaky"));
    budget.keeps("switcher", |chat| chat.switcher("c1"));
    let mut n = 0;
    budget.keeps("receive one line", |chat| {
        n += 1;
        chat.receive(busy::live(n))
    });
    budget.kept();
}
