//! What agents do, as every app draws it ([`zirc_proto::woke`]; the user's decisions, 23
//! and 24 Nov 2026): a wake as one line in the agent's own conversation, `Woke: build 612
//! failed on main`, with an edge in its tone's colour; an overlap two agents settled as one
//! line in the agents' room; and, once for each person, a note under their first of each
//! that says what it is, put away with Got it in any of their apps
//! ([`zirc_proto::onboarding::Note`]). Each is a [`Callout`] on its line: apps draw its
//! shape ([`super::RowShape`]) and decide nothing about it.
//!
//! A wake's line names whoever did it as this reader knows them, and adds what they said
//! only when this reader has the line that woke it: nothing reaches anyone here that they
//! couldn't read already.

use std::collections::HashSet;

use serde::Serialize;
use ts_rs::TS;
use zirc_proto::onboarding::Note;
use zirc_proto::woke::{self, By, Settled, Side, What, Woke};
use zirc_proto::{Event, ci};

use super::conversation::{Entry, EntryKind, agent};
use super::flat::RichSpan;
use super::rich::{Span, SpanKind};
use super::{Chat, strip_formatting};

/// The most of what someone said that a wake's line quotes, in characters.
pub const EXCERPT: usize = 80;

/// What a wake's line, a settled overlap's or a one-time note draws, as its row's shape
/// says ([`super::RowShape::Woke`], [`super::RowShape::Settled`], [`super::RowShape::Once`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct Callout {
    /// What it starts with, in bold: `Woke:`, `Overlap settled`, `This is a wake.`
    pub label: String,
    /// What follows the label: words, with agents' names in bold and files in monospace.
    pub spans: Vec<RichSpan>,
    /// The theme role its edge, or its icon, is drawn in ([`zirc_proto::woke::Tone::role`]):
    /// `error`, `ok`, `warning`, `agent` or `muted`.
    pub tone: String,
    /// For a one-time note, what its button says: `Got it`.
    pub button: Option<String>,
    /// What its button does, for the app to hand back to the core: for a one-time note,
    /// which it is, as zirc.dev names it (`first_wake`), which [`Chat::see_note`] marks seen.
    pub action: Option<String>,
    /// A command to run, in a monospace box of its own with a Copy button
    /// ([`super::COPY`]), after its words: a vacant role's `zirc role take fixer`.
    pub command: Option<String>,
    /// Words under [`Callout::command`]'s box, muted: who held the role last.
    pub footer: Option<String>,
    /// Whether its label heads its box, on a line of its own over its words (a settled
    /// overlap, getting started's prompt, a vacant role's card), decided here for every app;
    /// else it leads its words, in bold, as [`Callout::drawn`] has it (a wake, a one-time note).
    pub heads: bool,
}

impl Callout {
    /// It as one run of words to draw: its label in bold, a space, then what follows it.
    #[must_use]
    pub fn drawn(&self) -> Vec<RichSpan> {
        [bold(self.label.clone()), text(" ")]
            .into_iter()
            .chain(self.spans.iter().cloned())
            .collect()
    }

    /// What it says, whole, as one line: `Woke: build 612 failed on main`.
    #[must_use]
    pub fn said(&self) -> String {
        let words: String = self.spans.iter().map(|span| span.text.as_str()).collect();
        [
            Some(format!("{} {words}", self.label)),
            self.command.clone(),
            self.footer.clone(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
    }
}

pub(super) fn text(words: impl Into<String>) -> RichSpan {
    Span {
        text: words.into(),
        ..Span::default()
    }
    .into()
}

fn bold(words: impl Into<String>) -> RichSpan {
    Span {
        text: words.into(),
        bold: true,
        ..Span::default()
    }
    .into()
}

pub(super) fn code(words: impl Into<String>) -> RichSpan {
    Span {
        text: words.into(),
        kind: SpanKind::Code,
        ..Span::default()
    }
    .into()
}

/// What `entry` says, when it's a wake's line.
#[must_use]
pub fn woke_of(entry: &Entry) -> Option<Woke> {
    match &entry.kind {
        EntryKind::Zirc(message) => Woke::of(&message.event),
        _ => None,
    }
}

/// What `entry` says, when it's a settled overlap's line.
#[must_use]
pub fn settled_of(entry: &Entry) -> Option<Settled> {
    match &entry.kind {
        EntryKind::Zirc(message) => Settled::of(&message.event),
        _ => None,
    }
}

/// The id a note goes by on zirc.dev: `first_wake`.
fn note_id(note: Note) -> String {
    match serde_json::to_value(note) {
        Ok(serde_json::Value::String(id)) => id,
        _ => String::new(),
    }
}

impl Chat {
    /// Whoever a wake was by, as we know them: a person by their name, `you` for us; an
    /// agent session by what it's called, its owner's first when it isn't ours. `None` for
    /// CI and zirc, and for someone the wake doesn't name.
    fn woke_who(&self, woke: &Woke) -> Option<String> {
        let from = woke.from.as_deref()?;
        match woke.by {
            By::Person => Some(self.owner_name(from)),
            By::Agent => Some(self.side_name(from, woke.session.as_deref())),
            By::Ci | By::Zirc => None,
        }
    }

    /// The agent `session` on the machine `nick` as a line names it: what it's called, and
    /// whose it is first when it isn't ours, `Maya Chen's checkout`; the machine's agents
    /// as a whole, `Maya Chen's agent`, with no session.
    fn side_name(&self, nick: &str, session: Option<&str>) -> String {
        let owner = self.whose(nick).map(|(owner, _)| owner);
        let theirs = owner
            .as_deref()
            .filter(|owner| !owner.eq_ignore_ascii_case(self.me()))
            .map(|owner| self.owner_name(owner));
        match (session, theirs) {
            (Some(session), Some(owner)) => {
                format!(
                    "{} {}",
                    super::possessive(&owner),
                    self.session_label(session)
                )
            }
            (Some(session), None) => self.session_label(session),
            (None, Some(owner)) => super::agent_name(None, &owner).0,
            (None, None) => super::agent_name(None, super::YOU).0,
        }
    }

    /// What the line that woke it adds to a wake's words, when we have that line: what
    /// someone said in a channel, its first line cut to [`EXCERPT`]; for CI's, the targets
    /// that failed. Nothing for what was sent directly, which is the line above it for
    /// whoever may read it.
    fn woke_detail(&self, woke: &Woke) -> Option<String> {
        let entry = self
            .conversation(woke.channel.as_deref()?)?
            .entries
            .iter()
            .rev()
            .find(|entry| entry.id.is_some() && entry.id == woke.line)?;
        match (&entry.kind, woke.what) {
            (EntryKind::Zirc(message), What::Build) => {
                let Event::Custom { data, .. } = &message.event else {
                    return None;
                };
                let failed: Vec<String> = ci::Data::of(data)
                    .failed
                    .into_iter()
                    .map(|failed| failed.target)
                    .collect();
                (!failed.is_empty()).then(|| zirc_proto::clip(&failed.join(", "), EXCERPT))
            }
            (kind, What::Mentioned | What::Replied | What::Posted) => {
                let said = match kind {
                    EntryKind::Message(said) | EntryKind::Notice(said) => said.as_str(),
                    EntryKind::Zirc(message) => match &message.event {
                        Event::Text { body } => body.as_str(),
                        _ => return None,
                    },
                    _ => return None,
                };
                let line = strip_formatting(said.lines().next()?.trim());
                (!line.is_empty()).then(|| zirc_proto::clip(&line, EXCERPT))
            }
            _ => None,
        }
    }

    /// A wake's words after `Woke:`, as we read them: `Rajesh Iyer replied in #agents: the
    /// old listener lingers 200 ms`.
    #[must_use]
    pub fn woke_words(&self, woke: &Woke) -> String {
        let words = woke.words(self.woke_who(woke).as_deref());
        match self.woke_detail(woke) {
            Some(detail) => format!("{words}: {detail}"),
            None => words,
        }
    }

    /// What a wake's line draws.
    #[must_use]
    pub fn woke_callout(&self, woke: &Woke) -> Callout {
        Callout {
            label: woke::LABEL.to_owned(),
            spans: vec![text(self.woke_words(woke))],
            tone: woke.tone().role().to_owned(),
            button: None,
            action: None,
            command: None,
            footer: None,
            heads: false,
        }
    }

    /// A settled overlap's side as its line names it ([`Chat::side_name`]).
    fn settled_side(&self, side: &Side) -> String {
        // What its machine said it's called, until we've heard the session ourselves.
        if !self.session_names.contains_key(&side.session)
            && let Some(name) = &side.name
        {
            let owner = self
                .whose(&side.machine)
                .map(|(owner, _)| owner)
                .filter(|owner| !owner.eq_ignore_ascii_case(self.me()));
            return match owner {
                Some(owner) => format!("{} {name}", super::possessive(&self.owner_name(&owner))),
                None => name.clone(),
            };
        }
        self.side_name(&side.machine, Some(&side.session))
    }

    /// What a settled overlap's line draws, said in the repository `repo`: both agents in
    /// bold, the files in monospace, who lands first and whether it conflicts; the
    /// repository after it once the agents' room has heard more than one (the user's
    /// choice, 20 Nov 2026).
    #[must_use]
    pub fn settled_callout(&self, settled: &Settled, repo: Option<&str>) -> Callout {
        let first = self.settled_side(&settled.first);
        let second = self.settled_side(&settled.second);
        let [_, _, agreed, conflict] = settled.sentences(&first, &second);
        let mut spans = vec![
            bold(second.clone()),
            text(" and "),
            bold(first.clone()),
            text(" both change "),
        ];
        let (shown, more) = settled.listed();
        if shown.is_empty() {
            spans.push(text(settled.files()));
        }
        let last = shown.len() + usize::from(more.is_some());
        for (at, path) in shown.into_iter().enumerate() {
            if at > 0 {
                spans.push(text(if at + 1 == last { " and " } else { ", " }));
            }
            spans.push(code(path));
        }
        if let Some(more) = more {
            spans.push(text(format!(" and {more}")));
        }
        spans.push(text(format!("{agreed}{conflict}")));
        let several = self.presence.repositories(zirc_proto::AGENTS_CHANNEL) > 1;
        if let Some(repo) = repo.filter(|_| several) {
            spans.push(text(format!(" In {repo}.")));
        }
        Callout {
            label: woke::SETTLED_LABEL.to_owned(),
            spans,
            tone: woke::Tone::Person.role().to_owned(),
            button: None,
            action: None,
            command: None,
            footer: None,
            heads: true,
        }
    }

    /// Whether a settled overlap names one of our own agents.
    #[must_use]
    pub fn settled_ours(&self, settled: &Settled) -> bool {
        self.ours(&settled.first.machine) || self.ours(&settled.second.machine)
    }

    /// Takes the one-time notes our person has seen, as zirc.dev says
    /// ([`zirc_proto::onboarding::NOTES_PATH`]): until an app says, none is shown, as one
    /// seen on another device would show again. Whether it changed what shows.
    pub fn set_notes_seen(&mut self, seen: impl IntoIterator<Item = Note>) -> bool {
        let seen: HashSet<Note> = seen
            .into_iter()
            .chain(self.notes_seen.iter().flatten().copied())
            .collect();
        let changed = self.notes_seen.as_ref() != Some(&seen);
        self.notes_seen = Some(seen);
        changed
    }

    /// Marks the note called `note` (`first_wake`) seen, as its Got it does: it's gone here
    /// at once. Returns the note for the app to tell zirc.dev (`POST`
    /// [`zirc_proto::onboarding::NOTES_PATH`]), so it's gone in the person's other apps
    /// too; `None` when it names no note or was seen already.
    pub fn see_note(&mut self, note: &str) -> Option<Note> {
        let note = Note::ALL
            .into_iter()
            .find(|known| note_id(*known) == note)?;
        self.notes_seen
            .get_or_insert_with(HashSet::new)
            .insert(note)
            .then_some(note)
    }

    /// Whether `note` is still to show: an app said which were seen, and it isn't one.
    #[must_use]
    pub fn note_unseen(&self, note: Note) -> bool {
        self.notes_seen
            .as_ref()
            .is_some_and(|seen| !seen.contains(&note))
    }

    /// The first-wake note under `woke`, the newest wake's line in the conversation called
    /// `name`, when that's with one of our own agents and we haven't seen the note.
    #[must_use]
    pub fn first_wake_note(&self, name: &str, woke: &Woke) -> Option<Callout> {
        let (nick, session) = agent(name)?;
        if !self.ours(nick) || !self.note_unseen(Note::FirstWake) {
            return None;
        }
        Some(once(
            Note::FirstWake,
            &self.session_label(session),
            &woke.words(self.woke_who(woke).as_deref()),
        ))
    }

    /// The first-overlap note under `settled`, the newest settled overlap's line naming one
    /// of our agents, when we haven't seen the note.
    #[must_use]
    pub fn first_overlap_note(&self, settled: &Settled) -> Option<Callout> {
        (self.settled_ours(settled) && self.note_unseen(Note::FirstOverlap))
            .then(|| once(Note::FirstOverlap, "", ""))
    }
}

/// A one-time note's callout.
fn once(note: Note, agent: &str, what: &str) -> Callout {
    let (title, body) = woke::note_words(note, agent, what);
    Callout {
        label: title.to_owned(),
        spans: vec![text(body)],
        tone: woke::Tone::Quiet.role().to_owned(),
        button: Some(woke::GOT_IT.to_owned()),
        action: Some(note_id(note)),
        command: None,
        footer: None,
        heads: false,
    }
}

#[cfg(test)]
mod tests {
    use zirc_proto::Message;

    use super::*;
    use crate::chat::RowShape;
    use crate::chat::lines::tests::{at, chat, feed, named, utc, zirc};
    use crate::chat::lines::{Line, LineActions};

    const AGENTS: &str = zirc_proto::AGENTS_CHANNEL;
    const OURS: &str = "me-mbp/cc-1";

    /// What woke our agent in [`woken`]: alice's reply in the agents' room.
    fn reply() -> Woke {
        Woke {
            from: Some("alice".into()),
            channel: Some(AGENTS.into()),
            line: Some("p1".into()),
            ..Woke::new(What::Replied, By::Person)
        }
    }

    /// `chat()`, in the agents' room with our machine, where alice said something to our
    /// agent `checkout` (`cc-1`), which woke it, and it answered.
    fn woken() -> Chat {
        let mut chat = chat();
        feed(&mut chat, &[":me!u@h JOIN #agents"]);
        let said = ":alice!u@h PRIVMSG #agents :the old listener lingers 200 ms";
        let answer = Event::Text {
            body: "binding port 0 instead".into(),
        };
        feed(
            &mut chat,
            &[
                &at("10:00:00", Some("p1"), said),
                &at("10:00:01", Some("w1"), &ours(&reply().event())),
                &at("10:00:30", Some("a1"), &ours(&answer)),
            ],
        );
        chat
    }

    /// `event` from our agent `checkout` in the agents' room, as a line to feed.
    fn ours(event: &Event) -> String {
        zirc("me-mbp", AGENTS, &named("cc-1", "checkout", event.clone()))
    }

    /// Adds `settled`, said by `session` on `nick`, to the agents' room: straight in, as
    /// its event is longer than the one line a fed message gets here.
    fn settle(chat: &mut Chat, time: &str, (nick, session): (&str, &str), settled: &Settled) {
        let at = chat.position(AGENTS).unwrap();
        let mut entry = chat.conversations[at].entries[0].clone();
        entry.time = format!("2026-10-30T{time}Z").parse().unwrap();
        entry.id = Some(format!("s{time}"));
        entry.from = nick.into();
        entry.kind = EntryKind::Zirc(Message::new(session, settled.event()));
        chat.conversations[at].entries.push(entry);
    }

    #[test]
    fn a_wake_is_in_the_agents_own_conversation_and_says_what_its_reader_can_read() {
        let chat = woken();
        let woke: Vec<Woke> = chat
            .agent_activity(OURS)
            .iter()
            .filter_map(|line| woke_of(&line.entry))
            .collect();
        assert_eq!(woke, [reply()]);
        let callout = chat.woke_callout(&woke[0]);
        assert_eq!(
            callout.said(),
            "Woke: alice replied in #agents: the old listener lingers 200 ms"
        );
        assert_eq!(callout.tone, "warning");
        // A line said where we aren't is named, never quoted.
        let elsewhere = Woke {
            channel: Some("#secret".into()),
            ..reply()
        };
        assert_eq!(chat.woke_words(&elsewhere), "alice replied in #secret");
    }

    #[test]
    fn a_room_keeps_a_wake_to_everything_and_counts_none_unread() {
        let mut chat = woken();
        let shown = |chat: &Chat| {
            let agents = chat.conversation(AGENTS).unwrap();
            let entries = agents.entries.iter();
            entries
                .map(|entry| (agents.shows(entry), agents.counts(entry, "me")))
                .collect::<Vec<_>>()
        };
        // Alice's line and the agent's answer, never the wake.
        assert_eq!(shown(&chat), [(true, true), (false, false), (true, true)]);
        let agents = chat.conversation(AGENTS).unwrap();
        assert_eq!(
            agents.last_said(),
            agents.entries.last().map(|entry| entry.time)
        );
        assert!(!agents.entries[1].is_shown_in_list());
        chat.set_threshold(AGENTS, zirc_proto::Level::Debug);
        assert_eq!(shown(&chat)[1], (true, true));
    }

    #[test]
    fn a_wakes_edge_is_its_outcome_or_whose_doing_it_was() {
        let chat = woken();
        let failed = Woke {
            build: Some("612".into()),
            state: Some(zirc_proto::card::State::Failed),
            ..Woke::new(What::Build, By::Ci)
        };
        assert_eq!(chat.woke_callout(&failed).tone, "error");
        assert_eq!(chat.woke_callout(&failed).said(), "Woke: build 612 failed");
        let asked = Woke {
            from: Some("me-mbp".into()),
            session: Some("cc-1".into()),
            ..Woke::new(What::Asked, By::Agent)
        };
        assert_eq!(chat.woke_words(&asked), "checkout asked it");
        assert_eq!(chat.woke_callout(&asked).tone, "agent");
        let wrote = Woke {
            from: Some("me".into()),
            ..Woke::new(What::Wrote, By::Person)
        };
        assert_eq!(chat.woke_words(&wrote), "you wrote to it");
    }

    fn settled() -> Settled {
        let side = |machine: &str, session: &str, name: &str| Side {
            machine: machine.into(),
            session: session.into(),
            name: Some(name.into()),
        };
        Settled {
            first: side("bob-mbp", "cc-2", "tls-rotation"),
            second: side("me-mbp", "cc-1", "checkout"),
            paths: vec!["server/src/config.rs".into()],
            same_lines: false,
            agreed: zirc_proto::woke::Agreed::Order,
        }
    }

    #[test]
    fn a_settled_overlap_names_both_agents_and_is_in_each_ones_conversation() {
        let mut chat = woken();
        settle(&mut chat, "10:05:00", ("me-mbp", "cc-1"), &settled());
        let callout = chat.settled_callout(&settled(), None);
        assert_eq!(
            callout.said(),
            "Overlap settled checkout and tls-rotation both change server/src/config.rs. \
             Agreed: tls-rotation lands first; checkout rebases onto it. Different lines, no \
             conflict."
        );
        let mono: Vec<&str> = callout
            .spans
            .iter()
            .filter(|span| span.kind == crate::chat::RichSpanKind::Code)
            .map(|span| span.text.as_str())
            .collect();
        assert_eq!(mono, ["server/src/config.rs"]);
        assert_eq!(callout.spans.iter().filter(|span| span.bold).count(), 2);
        for agent in [OURS, "bob-mbp/cc-2"] {
            let activity = chat.agent_activity(agent);
            let said = activity
                .iter()
                .filter(|line| settled_of(&line.entry).is_some());
            assert_eq!(said.count(), 1, "{agent}");
        }
    }

    #[test]
    fn the_first_wakes_note_shows_once_an_app_says_its_unseen_and_goes_with_got_it() {
        let mut chat = woken();
        assert_eq!(chat.first_wake_note(OURS, &reply()), None);
        assert!(chat.set_notes_seen([Note::FirstOverlap]));
        // A teammate's agent's wake isn't our first.
        assert_eq!(chat.first_wake_note("bob-mbp/cc-2", &reply()), None);
        let note = chat.first_wake_note(OURS, &reply()).expect("the note");
        assert!(
            note.said().starts_with(
                "This is a wake. Alice replied in #agents, so Zirc woke checkout: no polling"
            ),
            "{}",
            note.said()
        );
        assert_eq!(note.button.as_deref(), Some("Got it"));
        assert_eq!(
            chat.see_note(note.action.as_deref().unwrap()),
            Some(Note::FirstWake)
        );
        assert_eq!(chat.first_wake_note(OURS, &reply()), None);
        assert_eq!(chat.see_note("first_wake"), None);
    }

    #[test]
    fn the_first_overlaps_note_is_for_one_that_names_our_agent() {
        let mut chat = woken();
        chat.set_notes_seen([]);
        let theirs = Settled {
            second: settled().first,
            first: Side {
                machine: "bob-mbp".into(),
                session: "cc-3".into(),
                name: None,
            },
            ..settled()
        };
        assert_eq!(chat.first_overlap_note(&theirs), None);
        let note = chat.first_overlap_note(&settled()).expect("the note");
        assert!(
            note.said()
                .starts_with("Your first overlap. Zirc saw two agents")
        );
        assert_eq!(chat.see_note("first_overlap"), Some(Note::FirstOverlap));
        assert_eq!(chat.first_overlap_note(&settled()), None);
    }

    fn shaped(lines: &[Line], shape: RowShape) -> Vec<&Line> {
        lines.iter().filter(|line| line.shape == shape).collect()
    }

    #[test]
    fn a_wake_is_a_row_of_its_own_and_its_answer_heads_a_new_run() {
        let chat = woken();
        let lines = chat.agent_lines(OURS, utc());
        let [woke] = shaped(&lines, RowShape::Woke)[..] else {
            panic!("one wake: {lines:#?}");
        };
        assert_eq!(
            woke.text,
            "Woke: alice replied in #agents: the old listener lingers 200 ms"
        );
        assert_eq!(
            woke.callout.as_ref().map(|callout| callout.tone.as_str()),
            Some("warning")
        );
        assert_eq!(
            (woke.speaker.as_ref(), woke.actions),
            (None, LineActions::default())
        );
        let answer = lines.last().unwrap();
        assert_eq!(
            (answer.text.as_str(), answer.continues),
            ("checkout: binding port 0 instead", false)
        );
        // The room keeps it to Everything.
        assert!(shaped(&chat.lines(AGENTS, utc(), false), RowShape::Woke).is_empty());
    }

    #[test]
    fn a_settled_overlap_is_a_row_in_the_room_and_in_each_agents_conversation() {
        let mut chat = woken();
        settle(&mut chat, "10:05:00", ("me-mbp", "cc-1"), &settled());
        let room = chat.lines(AGENTS, utc(), false);
        let [line] = shaped(&room, RowShape::Settled)[..] else {
            panic!("one settled overlap: {room:#?}");
        };
        assert!(
            line.text
                .starts_with("Overlap settled checkout and tls-rotation both change")
        );
        assert_eq!(line.callout, Some(chat.settled_callout(&settled(), None)));
        for agent in [OURS, "bob-mbp/cc-2"] {
            let lines = chat.agent_lines(agent, utc());
            assert_eq!(shaped(&lines, RowShape::Settled).len(), 1, "{agent}");
        }
    }

    /// The one-time note among `lines`, and the shape of the row it's under.
    fn once(lines: &[Line]) -> Option<(RowShape, &Line)> {
        let at = lines.iter().position(|line| line.shape == RowShape::Once)?;
        Some((lines[at - 1].shape, &lines[at]))
    }

    #[test]
    fn each_one_time_note_is_a_row_under_the_newest_line_its_about() {
        let mut chat = woken();
        settle(&mut chat, "10:05:00", ("me-mbp", "cc-1"), &settled());
        assert_eq!(once(&chat.agent_lines(OURS, utc())), None);
        chat.set_notes_seen([]);
        let ours = chat.agent_lines(OURS, utc());
        let (under, note) = once(&ours).expect("the first wake's note");
        assert_eq!(under, RowShape::Woke);
        assert_eq!(note.callout, chat.first_wake_note(OURS, &reply()));
        let room = chat.lines(AGENTS, utc(), false);
        let (under, note) = once(&room).expect("the first overlap's note");
        assert_eq!(under, RowShape::Settled);
        assert_eq!(note.callout, chat.first_overlap_note(&settled()));
        chat.see_note("first_wake");
        chat.see_note("first_overlap");
        assert_eq!(once(&chat.agent_lines(OURS, utc())), None);
        assert_eq!(once(&chat.lines(AGENTS, utc(), false)), None);
    }
}
