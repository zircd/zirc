//! A busy organisation, as the speed budget measures one ([`Size::BUSY`]): 145 channels
//! and 50 direct messages holding 11,000 lines, said by 20 people and 30 agents, with
//! intents, commits, CI's landings and their failure cards, mentions, links, code,
//! reactions and replies; and the same organisation a quarter of its size
//! ([`Size::QUARTER`]), which the budget compares it with.

use chrono::Duration;
use zirc_proto::card::{Action, Card, State, Who};

use super::*;

/// How big an organisation is.
#[derive(Debug, Clone, Copy)]
pub struct Size {
    /// The people, `p00` on.
    pub people: usize,
    /// The agents: one session on each machine, `p<owner>-m<n>`, two or so a person.
    pub agents: usize,
    /// The plain channels, `#c000` on, besides the repositories'.
    pub channels: usize,
    /// The repositories, `#r0` on, each a channel with its agents room under it.
    pub repos: usize,
    /// What the busiest channel, [`OPEN`], holds.
    pub busiest: usize,
    /// What every other conversation holds.
    pub each: usize,
}

impl Size {
    /// The busy organisation, its busiest channel holding as much as a conversation keeps.
    pub const BUSY: Self = Self {
        people: 20,
        agents: 30,
        channels: 140,
        repos: 5,
        busiest: 2000,
        each: 40,
    };

    /// The busy organisation a quarter of its size, near enough: a quarter of the people,
    /// agents and conversations, each as long, and its busiest a quarter as long, so a
    /// quarter of the lines in all.
    pub const QUARTER: Self = Self {
        people: 5,
        agents: 8,
        channels: 35,
        repos: 1,
        busiest: 500,
        each: 40,
    };

    fn person(self, n: usize) -> String {
        format!("p{:02}", n % self.people)
    }

    fn machine(self, n: usize) -> String {
        let n = n % self.agents;
        format!("{}-m{n}", self.person(n))
    }

    /// The three agents in channel number `c`, which speak there.
    fn agents_in(self, c: usize) -> [usize; 3] {
        [0, 1, 2].map(|k| (c + k * self.agents / 3) % self.agents)
    }

    /// Every conversation's name: the channels, the repositories and their agents rooms,
    /// the people's direct messages and the agents' machines'.
    pub fn conversations(self) -> Vec<String> {
        let mut names: Vec<String> = (0..self.channels).map(|c| format!("#c{c:03}")).collect();
        for r in 0..self.repos {
            names.push(format!("#r{r}"));
            names.push(format!("#r{r}/agents"));
        }
        names.extend((0..self.people).map(|p| self.person(p)));
        names.extend((0..self.agents).map(|a| self.machine(a)));
        names
    }
}

/// The channel the person has open, which holds [`Size::busiest`].
pub const OPEN: &str = "#c000";

/// An agent session's conversation that both sizes have.
pub const AGENT: &str = "p03-m3/cc-3";

/// `line` said at `at` with id `id`, in history `batch` if any.
fn tagged(mut line: Line, batch: Option<&str>, at: DateTime<Utc>, id: &str) -> Line {
    let mut tags = Vec::new();
    if let Some(batch) = batch {
        tags.push(("batch".to_owned(), Some(batch.to_owned())));
    }
    tags.push((
        "time".to_owned(),
        Some(at.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()),
    ));
    tags.push(("msgid".to_owned(), Some(id.to_owned())));
    tags.append(&mut line.tags);
    line.tags = tags;
    line
}

fn raw(text: &str) -> Line {
    text.parse().unwrap()
}

/// What a person says as message `m`: mostly a plain line, and now and then a mention of
/// us, a link or some code.
fn words(m: usize) -> String {
    match m % 50 {
        0 => format!("me: can you look at the build {m}?"),
        10 | 30 => format!("see https://example.com/issues/{m} for the trace"),
        20 | 40 => format!("run `bazel test //crates/x:{m}_test` again, it's flaky"),
        _ => format!("message {m} with some words about the release and the review"),
    }
}

/// What agent `a` says as its message `m`: an intent, a commit or a line of its own.
fn agent_says(a: usize, m: usize) -> Message {
    let event = match m % 3 {
        0 => Event::Intent {
            summary: format!("fix the flaky login test {m}"),
            paths: vec![format!("web/src/login{m}.ts"), "web/src/auth.ts".into()],
            expires: None,
            lasts: None,
        },
        1 => Event::Commit {
            branch: format!("fix-{a}"),
            sha: format!("{m:040}"),
            subject: format!("Fix the login page's focus {m}"),
            paths: vec!["web/src".into()],
            repo: None,
        },
        _ => Event::Text {
            body: format!("taking: the docs for {m}"),
        },
    };
    Message {
        name: Some(format!("task-{a}")),
        ..Message::new(format!("cc-{a}"), event)
    }
}

/// CI's landing `m` for agent `a`'s session: every fifth failed, with its card and buttons.
fn landing(a: usize, m: usize) -> Message {
    let failed = m.is_multiple_of(5);
    let mut data = serde_json::json!({
        "session": format!("cc-{a}"),
        "landing": format!("monorepo/{m}"),
        "state": if failed { "failed" } else { "passed" },
    });
    if failed {
        let card = Card {
            state: State::Failed,
            about: Some(format!("cc-{a}")),
            actions: vec![Action {
                id: "retry".into(),
                label: "Retry".into(),
                who: Who::default(),
                confirm: None,
            }],
            ..Card::new(format!("monorepo/{m}"), "//oss/web:unit failed")
        };
        data[zirc_proto::card::KEY] = serde_json::to_value(card).unwrap();
    }
    Message::new(
        "ci",
        Event::Custom {
            kind: zirc_proto::ci::LANDING_KIND.into(),
            data,
            text: format!(
                "cc-{a}: landing monorepo/{m} {}",
                if failed { "failed" } else { "landed" }
            ),
        },
    )
}

/// What's said in `channel` as its `count` lines of history, oldest first, the newest a
/// minute before noon: of every ten, seven people's, one an agent's (one of the three in
/// it, [`Size::agents_in`]), one CI's, and one a reaction or a reply to the line before.
fn channel_history(size: Size, channel: &str, c: usize, count: usize) -> Vec<Line> {
    let batch = format!("h-{}", channel.replace(['#', '/'], ""));
    let start =
        noon() - Duration::minutes(1) - Duration::seconds(i64::try_from(count).unwrap() * 97);
    let mut lines = vec![raw(&format!(":s BATCH +{batch} chathistory {channel}"))];
    for m in 0..count {
        let at = start + Duration::seconds(i64::try_from(m).unwrap() * 97);
        let id = format!("{batch}-{m}");
        let who = size.person(c + m);
        let agent = size.agents_in(c)[(m / 10) % 3];
        let line = match m % 10 {
            7 => line_from(&size.machine(agent), channel, &agent_says(agent, m)),
            8 => line_from("zirc-ci", channel, &landing(agent, m)),
            9 if m % 20 == 9 => raw(&format!(
                "@+draft/react=👍;+draft/reply={batch}-{} :{who}!u@h TAGMSG {channel}",
                m - 1
            )),
            9 => raw(&format!(
                "@+draft/reply={batch}-{} :{who}!u@h PRIVMSG {channel} :agreed, {}",
                m - 1,
                words(m)
            )),
            _ => raw(&format!(":{who}!u@h PRIVMSG {channel} :{}", words(m))),
        };
        lines.push(tagged(line, Some(&batch), at, &id));
    }
    lines.push(raw(&format!(":s BATCH -{batch}")));
    lines
}

/// A direct message's `count` lines: theirs and ours in turn, an agent's as its events.
fn direct_history(nick: &str, agent: Option<usize>, count: usize) -> Vec<Line> {
    let batch = format!("d-{nick}");
    let start =
        noon() - Duration::minutes(5) - Duration::seconds(i64::try_from(count).unwrap() * 301);
    let mut lines = vec![raw(&format!(":s BATCH +{batch} chathistory {nick}"))];
    for m in 0..count {
        let at = start + Duration::seconds(i64::try_from(m).unwrap() * 301);
        let id = format!("{batch}-{m}");
        let line = match (m % 2, agent) {
            (0, _) => raw(&format!(":me!u@h PRIVMSG {nick} :{}", words(m + 1))),
            (_, Some(a)) => {
                let mut line = line_from(nick, "me", &agent_says(a, m));
                line.source = Some(source(nick));
                line
            }
            (_, None) => raw(&format!(":{nick}!u@h PRIVMSG me :{}", words(m + 1))),
        };
        lines.push(tagged(line, Some(&batch), at, &id));
    }
    lines.push(raw(&format!(":s BATCH -{batch}")));
    lines
}

/// Who's in channel number `c`: us, eight people (or all, when fewer) and the three
/// machines whose agents speak there; the open one has everyone.
fn members(size: Size, c: usize) -> Vec<String> {
    if c == 0 {
        return (0..size.people)
            .map(|p| size.person(p))
            .chain((0..size.agents).map(|a| size.machine(a)))
            .collect();
    }
    (0..size.people.min(8))
        .map(|n| size.person(c + n))
        .chain(size.agents_in(c).map(|a| size.machine(a)))
        .collect()
}

/// An organisation of `size`: connected as `me`, in every channel, with each
/// conversation's history in, and [`OPEN`] selected.
pub fn organisation(size: Size) -> Chat {
    let mut chat = bare();
    chat.connected(link(true));
    let channels: Vec<String> = size
        .conversations()
        .into_iter()
        .filter(|name| is_channel(name))
        .collect();
    for (c, channel) in channels.iter().enumerate() {
        let mut lines = vec![raw(&format!(":me!u@h JOIN {channel}"))];
        for nick in members(size, c) {
            lines.push(raw(&format!(
                ":{} JOIN {channel} {nick} :{}",
                source(&nick),
                nick.to_uppercase()
            )));
        }
        let count = if c == 0 { size.busiest } else { size.each };
        lines.extend(channel_history(size, channel, c, count));
        for line in lines {
            chat.receive(line);
        }
    }
    for p in 0..size.people {
        for line in direct_history(&size.person(p), None, size.each) {
            chat.receive(line);
        }
    }
    for a in 0..size.agents {
        for line in direct_history(&size.machine(a), Some(a), size.each) {
            chat.receive(line);
        }
    }
    let open = chat.position(OPEN).expect("the open channel");
    let _ = chat.select(open);
    chat
}

/// One more line in [`OPEN`], live, as the person reads it: number `n`.
pub fn live(n: usize) -> Line {
    tagged(
        raw(&format!(":p{:02}!u@h PRIVMSG {OPEN} :{}", n % 5, words(n))),
        None,
        noon(),
        &format!("live-{n}"),
    )
}

#[test]
fn each_size_is_as_busy_as_it_says() {
    for (size, least) in [(Size::BUSY, 9000), (Size::QUARTER, 2000)] {
        let chat = organisation(size);
        let names: Vec<&str> = chat
            .conversations()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        for name in size.conversations().iter().chain([&AGENT.to_owned()]) {
            assert!(
                chat.conversation(name).is_some(),
                "{name} missing from {names:?}"
            );
        }
        let held: usize = chat.conversations().iter().map(|c| c.entries.len()).sum();
        println!("BUSY {} conversations, {held} entries", names.len());
        assert!(held >= least, "{held} entries");
        assert_eq!(chat.selected().name, OPEN);
    }
}
