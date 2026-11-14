//! How long the work an app repeats on every frame or every message takes with a big
//! workspace, printed with `--nocapture`. The limits are loose: they catch a quadratic
//! return, not noise.

use std::time::Instant;

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
