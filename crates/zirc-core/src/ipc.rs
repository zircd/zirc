//! The protocol between zircd and its clients over the Unix socket: one JSON object per
//! line. A client sends one [`Request`]. For most requests zircd answers with one
//! [`Response`] and closes; for [`Request::Watch`] it keeps sending
//! [`Response::Delivery`] until the client hangs up.
//!
//! The protocol has a version of its own, [`PROTOCOL`], apart from zirc's releases. A
//! request may say which it was written for, as `"v"` beside `"op"`, and one without is
//! for the first. zircd refuses a request for a version newer than its own, and
//! [`Status::protocol`] says which it speaks. Within a version, requests and answers only
//! gain fields, which readers that don't know them ignore; anything else is a new version,
//! and zircd answers the version before it the old way for at least one release.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt as _, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _,
    BufReader,
};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedReadHalf;
use zirc_proto::PROGRAM;
use zirc_proto::request::{Answer, Request as DecisionRequest};

pub use crate::presence::{
    Built, Claim, Conflict, Intent, Landing, LocalOverlap, Member, Overlap, Why, Worktree,
};

/// The version of the socket protocol this crate speaks. See the module's documentation.
pub const PROTOCOL: u32 = 1;

/// A state a session waits on changing ([`Request::WaitFor`]), such as `master` while it's
/// broken ([`zirc_proto::state`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StateWait {
    /// Its name, such as `master`.
    pub name: String,
    /// The channel of the repository it's in, such as `#monorepo`: any repository's when
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// How it stands as the session asks: a state said at any other level wakes it. Absent
    /// when it was never said, so the first said wakes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<zirc_proto::state::Level>,
}

/// How long after a session's `done:` or `dropped:` its landing still claims the task it
/// finished ([`Request::Task`]): 12 hours.
pub const LANDING_CLAIMS: u64 = 12 * 60 * 60;

/// What a client asks zircd.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Request {
    /// The connection, nick and channels.
    Status,
    /// Start watching the repository containing `path`, or, if it's watched already, add
    /// a session working in it.
    RepoAdd {
        /// Anywhere inside the repository.
        path: PathBuf,
        /// The agent session or person working in it, which signs its commits and pushes;
        /// `watcher` when unset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
    },
    /// Stop watching the repository containing `path`, or with `session`, only take that
    /// session out of it.
    RepoRemove {
        /// Anywhere inside the repository.
        path: PathBuf,
        /// The session to take out, leaving the repository watched for the rest.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
    },
    /// The repositories being watched.
    RepoList,
    /// Say something in a channel of the repository containing `repo`.
    Post {
        /// Anywhere inside a watched repository.
        repo: PathBuf,
        /// Who's saying it: an agent session, or a person.
        session: String,
        /// The text.
        text: String,
        /// Which of the repository's channels.
        #[serde(default)]
        to: Target,
        /// The short name apps show for an integration posting it, in place of a generic app
        /// badge: see [`zirc_proto::app_label`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app: Option<String>,
        /// A broadcast, for every session hearing it ([`zirc_proto::Message::everyone`]).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        everyone: bool,
        /// Post it to the repository's channel even though an agent session says it and it
        /// isn't for a person: `zirc post --force`. Left out when false, so an older zircd
        /// still understands it.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
        /// The id of the message it replies to (`+draft/reply`), whole or by a start of it as
        /// `zirc watch` shows it: it then goes where that message was said, whatever `to`
        /// says. Left out when absent, so an older zircd still understands a post without one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply: Option<String>,
    },
    /// [`Request::Post`] with parts to seal: each `||…||` in `text` goes sealed with the
    /// organisation's key ([`crate::sealed`]), `[sealed]` in its place. A request of its own,
    /// so an older zircd, which can't seal, refuses it rather than posting the parts as they
    /// are.
    SealedPost {
        /// Anywhere inside a watched repository.
        repo: PathBuf,
        /// Who's saying it: an agent session, or a person.
        session: String,
        /// The text, its parts to seal between `||` markers.
        text: String,
        /// Which of the repository's channels.
        #[serde(default)]
        to: Target,
        /// As [`Request::Post`]'s.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
        /// As [`Request::Post`]'s.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply: Option<String>,
    },
    /// Opens part `part`, counted from 1, of the message `message` names, whole or by a start
    /// of its id, with this machine's keys: answered with [`Response::Unsealed`].
    Unseal {
        /// The message's id, or a start of it.
        message: String,
        /// Which of its sealed parts, from 1.
        part: usize,
    },
    /// Say something to one person alone, as a direct message from `session`: to `to`, or
    /// without it to this machine's owner, which is how an agent answers the person whose
    /// machine it's on.
    Dm {
        /// Who's saying it: an agent session.
        session: String,
        /// The text.
        text: String,
        /// Whom to: a person's nick or account. This machine's owner when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
    },
    /// Leave a note for one agent session, as `session` ([`zirc_proto::Event::Note`]): the
    /// one `to` names, by its id or the name it goes by, or as `<machine>/<session>`,
    /// wherever it works. Its watch alone hears it, if its machine lets this one query it
    /// ([`crate::agents`]). Answered with [`Response::Noted`].
    Note {
        /// Who's writing: an agent session.
        session: String,
        /// The session it's for: `<session>`, its name, or `<machine>/<session>`.
        to: String,
        /// What it says.
        text: String,
    },
    /// Ask agents something, as `session` ([`zirc_proto::Event::Query`]): those of the
    /// machine `to` names, `<machine>/<session>` for one of them, or a person's nick for
    /// those of each of their machines here. Their watches hear it if their machine lets
    /// this one query it ([`crate::agents`]). Answered with [`Response::Asked`].
    Ask {
        /// Who's asking: an agent session.
        session: String,
        /// Whom: `<machine>`, `<machine>/<session>` or a person's nick.
        to: String,
        /// The question.
        text: String,
    },
    /// Record a decision this machine's owner made ([`zirc_proto::decision`]): in the
    /// organisation's `#ops` and the channel of the repository containing `repo`. With
    /// `session`, the agent session it was said to relays it; without, the person is at a
    /// terminal here, as only `zirc decide` run by a person says. Answered with
    /// [`Response::Decided`].
    Decide {
        /// Anywhere inside a watched repository.
        repo: PathBuf,
        /// The session relaying it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
        /// The owner's words, verbatim.
        quote: String,
        /// What it covers.
        about: String,
        /// Where to read more.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        link: Option<String>,
        /// The sessions and branches it's for, whose watches it wakes.
        #[serde(rename = "for", default, skip_serializing_if = "Vec::is_empty")]
        for_: Vec<String>,
        /// The server's id of the earlier decision it takes the place of.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replaces: Option<String>,
    },
    /// Say how something in the repository containing `repo` stands now
    /// ([`zirc_proto::state`]), such as its default branch broken and who's fixing it: in
    /// the organisation's `#ops`, where `zirc state` reads it. Answered with
    /// [`Response::Channel`].
    SetState {
        /// Anywhere inside a watched repository.
        repo: PathBuf,
        /// Who's saying it: an agent session, a person, or a script.
        session: String,
        /// What it's the state of, such as `master`.
        name: String,
        /// How it stands.
        level: zirc_proto::state::Level,
        /// Why, in a few words.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<String>,
        /// Who's on it, whose watch it wakes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
        /// Where to read more.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        link: Option<String>,
        /// When it ends by itself, as a time tag: zircd says it's fine then.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<String>,
    },
    /// Ask this machine's owner `question` in `#decisions` ([`zirc_proto::request`]), to
    /// answer with a reaction: answered with [`Response::Requested`], and their answer, once
    /// they give it, comes to `session`'s watch as [`Response::Answered`].
    AskOwner {
        /// The session asking.
        session: String,
        /// The question, one line.
        question: String,
        /// What a yes does.
        yes: String,
        /// What a no does.
        no: String,
        /// The answer the session recommends.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recommend: Option<Answer>,
        /// Whether acting on a yes can't be undone.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        irreversible: bool,
    },
    /// Answer the query `query`, heard while watching, as `session`.
    Reply {
        /// Who's answering: an agent session.
        session: String,
        /// The query's id, as the watch said it.
        query: String,
        /// The answer.
        text: String,
    },
    /// Who may query this machine's agents, set to `set` if given. Answered with
    /// [`Response::Queries`].
    Queries {
        /// The new setting.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        set: Option<crate::agents::Queries>,
    },
    /// Send a team's own kind of event to a channel of the repository containing `repo`,
    /// as [`zirc_proto::Event::Custom`].
    Emit {
        /// Anywhere inside a watched repository.
        repo: PathBuf,
        /// Who's sending it: an agent session, a person, or a script.
        session: String,
        /// Its kind, such as `x-acme/deploy`: see [`zirc_proto::check_kind`].
        kind: String,
        /// What it carries, if anything.
        #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
        data: serde_json::Value,
        /// What it says, for people.
        text: String,
        /// Which of the repository's channels.
        #[serde(default)]
        to: Target,
        /// The short name apps show for an integration posting it, in place of a generic app
        /// badge: see [`zirc_proto::app_label`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app: Option<String>,
    },
    /// Send something for the organisation's workflows ([`zirc_proto::workflow`]) to
    /// `channel`, in the organisation of the repository containing `repo`: a workflow's file
    /// to `#workflows`, what a worker did with a step ([`zirc_proto::run::StepAction`]), or
    /// words to a workflow's nick, as [`zirc_proto::Event::Text`].
    Workflow {
        /// Anywhere inside a watched repository.
        repo: PathBuf,
        /// Who's sending it: an agent session or a person.
        session: String,
        /// Where: `#workflows`, or the workflow's channel.
        channel: String,
        /// What.
        event: zirc_proto::Event,
    },
    /// Tell the repository's channel what the session is about to touch.
    Intend {
        /// Anywhere inside a watched repository.
        repo: PathBuf,
        /// The session about to start.
        session: String,
        /// What it's about to do, in a line.
        summary: String,
        /// The paths or directories it expects to touch: absolute, or relative to `repo`.
        paths: Vec<PathBuf>,
        /// How many seconds until it ends unless renewed.
        seconds: u64,
    },
    /// React to a message with an emoji, as people's apps do (`IRCv3`'s `+draft/react`),
    /// from `session`: the message `message` names, or the latest from `last`.
    React {
        /// Where it's asked, which names the repository and so the organisation.
        repo: PathBuf,
        /// The session reacting.
        session: String,
        /// The message's id, or the start of one, as `zirc watch` shows it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        /// Instead, the latest message heard from this nick or session.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last: Option<String>,
        /// Where `message` was said, a channel or a nick, for one zircd hasn't heard lately,
        /// such as from `zirc search`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
        /// The emoji.
        emoji: String,
        /// Take the reaction back rather than give it.
        #[serde(default)]
        remove: bool,
    },
    /// Show `to` that the session is typing, as people's apps do, until it says something
    /// there or `seconds` pass: never stored, never shown by `zirc watch`.
    Typing {
        /// Where the command runs, which names the organisation on a machine signed in to
        /// several.
        repo: PathBuf,
        /// The session that's typing.
        session: String,
        /// A person's nick or a channel.
        to: String,
        /// How many seconds at most.
        seconds: u64,
    },
    /// Join a topic channel: `#<repo>/<topic>`, or `#<topic>` across the organisation.
    Join {
        /// Anywhere inside a watched repository, which names the repository's topics.
        repo: PathBuf,
        /// The topic.
        topic: String,
        /// Whether it's the organisation's topic rather than the repository's.
        org: bool,
        /// The session joining, whose watch then hears the topic's posts; without one,
        /// the machine joins, and only watchers that aren't a session hear them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
    },
    /// Name `session`, as apps show it in place of its id, say which agent tool it runs in,
    /// or pick its picture: zircd puts them on what the session sends from now on
    /// ([`zirc_proto::Message::name`], [`zirc_proto::Message::tool`],
    /// [`zirc_proto::Message::avatar`]), and keeps them
    /// across restarts. Without a name, a session goes by the branch it named first, if
    /// it did. What's left out stays as it was.
    Name {
        /// The session.
        session: String,
        /// Its name, made plain by [`zirc_proto::session_name`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// The agent tool it runs in, made plain by [`zirc_proto::session_tool`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool: Option<String>,
        /// The picture it goes by: one of [`zirc_proto::AGENT_AVATARS`], or refused.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        avatar: Option<String>,
        /// Its full title, as the agent tool shows it: sent to the session's channels as a
        /// [`zirc_proto::session::SessionCard`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// Where its owner opens it, such as a `claude://` link: kept on this machine, and
        /// only ever given over this socket ([`Request::LocalSessions`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        link: Option<String>,
    },
    /// Join or leave one of the workflows' roles ([`zirc_proto::workflow::Who::Role`]) as
    /// `session`: the machine says each live session's roles with the list of them
    /// ([`zirc_proto::session::Live`]), so a workflow gives a role's steps to its members.
    /// A session's roles end with its `done:` or `dropped:`, or once it's no longer live.
    /// Answered with [`Response::Roles`].
    Role {
        /// The session.
        session: String,
        /// The role, as [`zirc_proto::workflow::check_role`] takes it.
        role: String,
        /// Joining it, or leaving it.
        join: bool,
    },
    /// This machine's named sessions, with their titles and links, for its owner's apps:
    /// answered with [`Response::LocalSessions`].
    LocalSessions,
    /// The organisation's sessions with a watch open now, on any machine, as each machine says
    /// its list ([`zirc_proto::session::Live`]), and those seen live since zircd started that
    /// no longer are: whether a state's owner is still on it. Answered with
    /// [`Response::LiveSessions`]; a zircd from before it says it doesn't know the request.
    LiveSessions,
    /// Leave a topic channel joined with [`Request::Join`].
    Leave {
        /// As for [`Request::Join`].
        repo: PathBuf,
        /// As for [`Request::Join`].
        topic: String,
        /// As for [`Request::Join`].
        org: bool,
        /// The session leaving: the channel is left once no session or person is in it.
        /// Without one, the machine leaves it, whoever joined.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
    },
    /// Who is working on what in the channels joined, this machine included.
    Who,
    /// The latest messages in a channel or a private conversation, from the server's
    /// history, each with the id [`Request::Trim`] names, between `since` and `until` when
    /// given, read back a page at a time ([`crate::history`]). Answered with
    /// [`Response::Messages`].
    Read {
        /// The channel, or the nick of a private conversation with this machine.
        target: String,
        /// How many, at most: the newest.
        limit: usize,
        /// The oldest moment, in seconds since the Unix epoch, inclusive.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<u64>,
        /// The newest moment, in seconds since the Unix epoch, exclusive.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<u64>,
    },
    /// The private conversations with this machine that had messages in the last `seconds`,
    /// as the server keeps them: [`Response::Conversations`].
    Conversations {
        /// How far back.
        seconds: u64,
    },
    /// Remove one message from the server's history ([`crate::trim`]), waiting until the
    /// server has or says why not: [`Response::Done`], or an error saying why. The server
    /// allows it for messages this machine's account sent, and for any in a channel it's an
    /// operator of.
    Trim {
        /// The channel, or the nick of the private conversation, it was sent to.
        target: String,
        /// The message's id, as [`Delivery::id`] and [`Response::Messages`] give it.
        id: String,
        /// Why, which the server passes on to those it tells.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Stream every zirc message from the channels being watched, and overlaps with this
    /// machine's own uncommitted changes as they appear, each with why it concerns
    /// `session`, if it does.
    Watch {
        /// The session watching: an agent session working here, or none for the machine's
        /// person.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
    },
    /// How this machine's sessions' watches woke over the last `days` days, and how often
    /// the session acted soon after: answered with [`Response::Wakes`]. Counts only.
    Wakes {
        /// How many days back, today included.
        #[serde(default = "a_week")]
        days: u32,
    },
    /// Let wakes through again at once, the circuit breaker on them being open, and count
    /// afresh: answered with [`Response::Wakes`].
    WakesResume,
    /// The task `session` is on, the id zircd gave it when it posted `taking:` or, on none,
    /// stated an intent ([`zirc_proto::Message::task`]): answered with [`Response::Task`].
    Task {
        /// The session.
        session: String,
        /// For a landing (`//ops:land`), which claims the task: the one the session is on,
        /// else the one it finished last, if in the last [`LANDING_CLAIMS`] seconds and no
        /// landing claimed it yet, since sessions often post `done:` before they land. A task
        /// an intent started, with no `taking:`, ends with its landing.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        landing: bool,
    },
    /// Note that `session` waits on each of `on`, another session, a branch or a build by its
    /// number, so CI's landing line naming it wakes the session ([`Why::Awaited`]) until it
    /// lands, fails or conflicts, or for three hours; and on each of `states` changing, so
    /// the first state said of it at another level wakes it ([`Why::StateChanged`]), once.
    /// With nothing in either, it says what the session waits on. Answered with
    /// [`Response::Awaits`].
    WaitFor {
        /// The session waiting.
        session: String,
        /// What it waits on.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        on: Vec<String>,
        /// The states it waits on changing.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        states: Vec<StateWait>,
        /// The reactions it waits for, such as 👀: the machine's person giving one to a
        /// message wakes the session ([`Why::ReactionAwaited`]), once a message, however
        /// often it's given. Each lasts a day from when it was said or last woke the session.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        reactions: Vec<String>,
        /// Stop waiting for `reactions`, rather than start.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        drop: bool,
    },
    /// Note that `session`'s branch is stacked on `branch`, another session's not yet
    /// landed, whose head was `tip` as the session built on it: CI's line saying it landed
    /// wakes the session ([`Why::BranchLanded`]), as does a push that moves it
    /// ([`Why::Landing`]), and the stack is kept, with `tip`, for the rebase onto the default
    /// branch that follows. With `drop`, forget the stack on `branch`; with no `branch`, only
    /// say the session's stacks. Answered with [`Response::Stacks`].
    Stack {
        /// The session whose branch is stacked.
        session: String,
        /// The branch it's stacked on.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        /// That branch's head as the session built on it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tip: Option<String>,
        /// Forget the stack on `branch` instead.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        drop: bool,
    },
    /// Stop zircd, after answering.
    Stop,
    /// Start zircd again from its binary, after answering, as after an update: watches are
    /// told [`RESTARTING`] and watch again once it's back.
    Restart,
    /// Ask for the lease `name` for `session` ([`crate::leases`]), or, holding it, hold it
    /// for `seconds` from now: while others wait, no further than [`crate::leases::renewal`]
    /// allows, past which it's asked for again behind them. Answered with
    /// [`Response::Leases`] holding that lease.
    Lease {
        /// The lease, such as `emulator`.
        name: String,
        /// The session asking.
        session: String,
        /// How long to hold it once it's theirs: [`crate::leases::DEFAULT_SECONDS`] unless
        /// said, at most [`crate::leases::MAX_SECONDS`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seconds: Option<u64>,
        /// The asking `zirc` command's process id: zircd gives the lease back once nothing
        /// heavy has run for a while under whoever ran it, such as the session's agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        /// Whether the asking command waits until the lease is the session's (`zirc lease
        /// --wait`): while `pid` runs, it says so itself, so the session's watch isn't told.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        wait: bool,
    },
    /// Give back the lease `name`, or stop waiting for it, as `session`. Answered with
    /// [`Response::Leases`] holding that lease.
    LeaseRelease {
        /// The lease.
        name: String,
        /// The session holding or waiting for it.
        session: String,
    },
    /// Every lease held or waited for. Answered with [`Response::Leases`].
    Leases,
}

/// Which channel a post goes to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Target {
    /// The repository's channel, where every session working on it is.
    #[default]
    Repo,
    /// Where a post goes when it doesn't say ([`crate::team`]): the repository's agents
    /// channel, `#<repo>/agents`, unless the team names another room, or the repository's
    /// channel for a post that's for a person. Anywhere but a post, the repository's channel.
    Default,
    /// The channel of the branch checked out, which only a branch that's been pushed has.
    Branch,
    /// The organisation's channel, `#general`.
    Org,
    /// A topic channel, which must have been joined.
    Topic {
        /// The topic.
        name: String,
        /// Whether it's the organisation's topic rather than the repository's.
        org: bool,
    },
}

/// What zircd answers.
#[allow(
    clippy::large_enum_variant,
    reason = "a delivery is much larger than most answers, but each lives only as long as it's sent; allowed, not expected, as a growing Status is sometimes as large"
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Response {
    /// The answer to [`Request::Status`].
    Status(Status),
    /// The answer to [`Request::RepoList`], and to adding or removing one.
    Repos {
        /// The repositories being watched, sorted by path.
        repos: Vec<Repo>,
    },
    /// Done, with nothing to say.
    Done,
    /// The channel a post, join or leave was about.
    Channel {
        /// Its name.
        channel: String,
        /// The post's own link, which opens it in every app
        /// ([`zirc_proto::link::conversation_link`]), once the server's echo gave its id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        link: Option<String>,
    },
    /// The answer to [`Request::Who`].
    Who {
        /// Each joined channel's working sets, sorted by channel.
        channels: Vec<ChannelPresence>,
    },
    /// A session's stacks: the answer to [`Request::Stack`].
    Stacks {
        /// The branches it's stacked on, oldest first.
        stacks: Vec<Stack>,
    },
    /// What a session waits on: the answer to [`Request::WaitFor`].
    Awaits {
        /// Sessions, branches and builds by their number.
        on: Vec<String>,
    },
    /// The answer to [`Request::Read`].
    Messages {
        /// The messages, oldest first.
        messages: Vec<Delivery>,
    },
    /// The answer to [`Request::Conversations`].
    Conversations {
        /// The nicks of the people and machines each conversation is with.
        targets: Vec<String>,
    },
    /// One message, while watching.
    Delivery(Delivery),
    /// While watching: this machine's owner answered a session's request.
    Answered(Box<Answered>),
    /// While watching: a message sent to this machine's nick rather than to a channel.
    Direct(Direct),
    /// While watching: someone reacted to a message, or took a reaction back.
    Reaction(Reaction),
    /// The answer to [`Request::Role`]: the session's roles now.
    Roles {
        /// Its roles, sorted.
        roles: Vec<String>,
    },
    /// The answer to [`Request::LiveSessions`].
    LiveSessions {
        /// The sessions live now, sorted.
        live: Vec<String>,
        /// Those seen live since zircd started that no longer are, sorted.
        gone: Vec<String>,
    },
    /// The answer to [`Request::LocalSessions`], by session.
    LocalSessions {
        /// Each session given a name, a title or a link here.
        sessions: Vec<LocalSession>,
    },
    /// The answer to [`Request::Wakes`], oldest day first.
    Wakes {
        /// A count for each day and reason a watch woke.
        counts: Vec<WakeCount>,
        /// The circuit breaker on wakes, from a zircd that has one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        breaker: Option<BreakerState>,
    },
    /// The answer to [`Request::Task`].
    Task {
        /// The task's id, while the session is on one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task: Option<String>,
    },
    /// The answer to [`Request::AskOwner`].
    Requested {
        /// The request's id, which its answer names.
        id: String,
        /// Where it was asked.
        channel: String,
        /// The answer taken at once, the session's own recommendation, on a machine set to
        /// take them (`accept_recommended`): the session acts on it now, and hears on its
        /// watch if its person later says otherwise.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        taken: Option<Answer>,
    },
    /// The answer to [`Request::Decide`].
    Decided {
        /// The channels it was posted to.
        channels: Vec<String>,
    },
    /// The answer to [`Request::Note`], and to a [`Request::Post`] addressed to one agent
    /// session, which goes to it as a note.
    Noted {
        /// Where it went: `<machine>/<session>`.
        to: String,
    },
    /// The answer to [`Request::Ask`].
    Asked {
        /// The query's id, which its answers name.
        id: String,
        /// The machines it went to: this one's own nick among them when it was for its
        /// own sessions.
        to: Vec<String>,
    },
    /// The answer to [`Request::Queries`].
    Queries {
        /// Who may query this machine's agents.
        queries: crate::agents::Queries,
        /// Whether the organisation keeps other members' agents out whatever this machine
        /// says, so `anyone` takes only its owner's own (`zirc_proto::colleagues`).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        colleagues_kept_out: bool,
    },
    /// While watching: someone else started changing paths this machine has changed too.
    Overlap(Overlap),
    /// While watching: two worktrees on this machine started changing the same paths.
    LocalOverlap(LocalOverlap),
    /// The answer to [`Request::Unseal`]: the part's value.
    Unsealed {
        /// What was sealed.
        value: String,
    },
    /// While watching: a push changes the same lines as a worktree on this machine.
    Conflict(Conflict),
    /// The answer to [`Request::Lease`], [`Request::LeaseRelease`] and [`Request::Leases`].
    Leases {
        /// The leases, by name.
        leases: Vec<crate::leases::Lease>,
    },
    /// The request failed.
    Error {
        /// Why, for a person to read.
        message: String,
    },
}

impl Response {
    /// [`Response::Channel`] about `channel`, without a link.
    #[must_use]
    pub fn channel(channel: impl Into<String>) -> Self {
        Self::Channel {
            channel: channel.into(),
            link: None,
        }
    }
}

/// A branch a session's branch is stacked on ([`Request::Stack`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Stack {
    /// The branch.
    pub branch: String,
    /// Its head as the session built on it: what a rebase leaves behind.
    pub tip: String,
    /// Whether CI said it landed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub landed: bool,
    /// Its head since, when a push moved it from `tip`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved: Option<String>,
    /// Whether it was deleted from origin without landing: dropped, not to be landed in its
    /// place.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dropped: bool,
}

/// The daemon's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Status {
    /// Whether the IRC connection is up.
    pub connected: bool,
    /// The server, as `host:port`.
    pub server: String,
    /// The nick on the server, while connected.
    pub nick: Option<String>,
    /// The channels joined, sorted.
    pub channels: Vec<String>,
    /// Why zircd isn't connecting, when it's stopped trying until something changes, such
    /// as [`REVOKED`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    /// The sessions with a watch running, sorted, `None` for a watch without one: what
    /// `zirc watch` started, and whether it's still there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watching: Vec<Option<String>>,
    /// The socket protocol zircd speaks, [`PROTOCOL`]; 0 from a zircd that predates
    /// versions, which speaks version 1.
    #[serde(default)]
    pub protocol: u32,
    /// zircd's version. zircds from before 0.1.11 don't say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Each organisation's connection, when this machine is signed in to several: the
    /// fields above are then the first's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orgs: Vec<OrgStatus>,
    /// The circuit breaker on wakes, from a zircd that has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breaker: Option<BreakerState>,
    /// When the connection is a guest's terminal: the public channel it's a guest of, as
    /// `zirc login` names it, such as `acme/web` ([`crate::guest::of`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest: Option<String>,
    /// The longest zircd was busy with one thing in the last minute or two, in milliseconds:
    /// how long a command can have waited for it. From a zircd that measures it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy_ms: Option<u64>,
}

/// One organisation's connection, in [`Status::orgs`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OrgStatus {
    /// The organisation.
    pub name: String,
    /// Whether its connection is up.
    pub connected: bool,
    /// Its server, as `host:port`.
    pub server: String,
    /// The nick on its server, while connected.
    pub nick: Option<String>,
    /// The channels joined there, sorted.
    pub channels: Vec<String>,
    /// Why zircd isn't connecting to it, when it's stopped trying.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    /// When it's a guest's terminal: the public channel it's a guest of, as [`Status::guest`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest: Option<String>,
}

/// What zircd says when the web app no longer takes this machine's account: it's stopped
/// connecting until the machine logs in again.
pub const REVOKED: &str =
    "this machine's account was renamed or revoked; run `zirc login` to set it up again";

/// What zircd tells watches as it starts again, as after an update: they watch again once
/// it's back.
pub const RESTARTING: &str = "zircd is restarting";

/// A watched repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Repo {
    /// The root of its working tree.
    pub path: PathBuf,
    /// The channel its work goes to.
    pub channel: String,
    /// Every session added with it, the last one added last.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<RepoSession>,
    /// The organisation it belongs to, whose server alone hears about it. zircds from
    /// before several organisations don't say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    /// Sessions registered here in the last day that another worktree, still there, had
    /// registered: two sessions under one name, which `zirc` warns of (decided 8 Nov
    /// 2026: a warning, never a refusal). zircds from before it don't say.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared: Vec<Shared>,
}

/// A session one worktree registered that another, still there, had registered before it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Shared {
    /// The session.
    pub session: String,
    /// The other worktree's root.
    pub with: PathBuf,
    /// When, in seconds since the Unix epoch.
    pub at: u64,
}

/// What `zirc` says of `shared`, a session `here` took from another worktree.
#[must_use]
pub fn shared_warning(shared: &Shared, here: &str, with: &str) -> String {
    format!(
        "warning: {} is registered in {here} now, and was in {with} too: two sessions under \
         one name hear each other's wakes and are credited with each other's landings; give \
         each session its own --session",
        shared.session
    )
}

/// A session working in a watched repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RepoSession {
    /// The session.
    pub session: String,
}

/// The working sets in one channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelPresence {
    /// The channel.
    pub channel: String,
    /// The organisation whose channel it is, when this machine is signed in to several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    /// Each machine with uncommitted changes, by nick.
    pub members: Vec<Member>,
    /// What sessions have said they're taking on, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claims: Vec<Claim>,
    /// What sessions have said they're about to touch, and haven't finished or let expire.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intents: Vec<Intent>,
    /// The latest pushes to the default branch, newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub landings: Vec<Landing>,
    /// What each worktree's work is built on and changes, for those whose machines say.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub built: Vec<Built>,
}

/// A message someone sent to a channel: a zirc one, or plain text a person typed in an IRC
/// client, which arrives as a text event with an empty session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Delivery {
    /// The channel it was sent to.
    pub channel: String,
    /// The nick it came from: the sending machine, or the person.
    pub from: String,
    /// The message.
    pub message: zirc_proto::Message,
    /// The server's id for it (`IRCv3`'s `msgid`), which [`Request::Trim`] names. This
    /// machine's own posts, which the server doesn't echo back, come without one: find
    /// theirs with [`Request::Read`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// When the server says it was sent, in RFC 3339, when it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    /// Why it concerns the watching session, if it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<Why>,
    /// The id of the message it replies to (`IRCv3`'s `+draft/reply`), if it's a reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
    /// The organisation whose server it came from, while watching on a machine signed in
    /// to several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    /// The account the server says sent it (`account-tag`), when it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Whose machine the server says sent it, by its host (`machine/<owner>`,
    /// [`crate::agents::host_owner`]): None for a person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosted_by: Option<String>,
    /// Whose own client the server says sent it, by its host (`client/<person>`,
    /// [`zirc_proto::names::client_host`]): a person signed in themself under another name,
    /// as in an IRC client, whose word it is as their own account's is. None otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_of: Option<String>,
    /// For a decision ([`zirc_proto::decision`]): how sure it is the decider's own word, as
    /// this machine can tell from [`Delivery::account`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided: Option<Box<crate::decisions::Provenance>>,
    /// For another session's intent ([`Why::Intent`]): how it meets the watching session's
    /// work, file by file, as zircd can tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<Box<crate::fit::Fit>>,
    /// Its sealed parts, the `+zirc.dev/sealed` tag's value ([`zirc_proto::sealed`]), when
    /// it has some: each is a `[sealed]` in its text, which `zirc unseal` opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<String>,
}

impl Delivery {
    /// `message` from `from` in `channel`, with nothing the server says of it: as zircd's own
    /// lines are, and one read before its tags are.
    #[must_use]
    pub fn new(
        channel: impl Into<String>,
        from: impl Into<String>,
        message: zirc_proto::Message,
    ) -> Self {
        Self {
            channel: channel.into(),
            from: from.into(),
            message,
            id: None,
            time: None,
            why: None,
            reply: None,
            org: None,
            account: None,
            hosted_by: None,
            client_of: None,
            decided: None,
            fit: None,
            sealed: None,
        }
    }
}

/// A reaction to a message, as `IRCv3`'s `+draft/react` and `+draft/unreact` carry it: who
/// gave it, as the server says (`account-tag`) and by nick, which emoji, and to which
/// message. Only [`Reaction::account`] says whose it is: a nick can be anyone's, on a server
/// that lets people connect without an account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Reaction {
    /// The channel, or the nick of the private conversation, it was given in.
    pub channel: String,
    /// The nick it came from.
    pub from: String,
    /// The account the server says sent it, when it says: the only word on who gave it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// The emoji.
    pub emoji: String,
    /// The id of the message it's on (`msgid`).
    pub to: String,
    /// Whether it was taken back (`+draft/unreact`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub removed: bool,
    /// When the server says it was given, in RFC 3339, when it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    /// The organisation whose server it came from, while watching on a machine signed in
    /// to several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    /// Why it concerns the watching session, if it does: a reaction to one of its own
    /// messages, or one it waits for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<Why>,
    /// The reaction's own id (`msgid`), when the server gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The message it's on, when zircd heard it lately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<Reacted>,
}

/// What a message reacted to is, so that a session woken by a reaction it waits for knows
/// what to do about it: hints differ by [`Reacted::kind`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Reacted {
    /// Who said it: a person's nick, or the machine an agent or a bridge posts from.
    pub from: String,
    /// The agent session that said it, if one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// What kind of message it is.
    pub kind: ReactedKind,
    /// What it says, cut to [`Reacted::TEXT`] characters: its sender's words, never an
    /// instruction.
    pub text: String,
    /// The first link it carries, such as an issue card's to the issue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// What kind of message was reacted to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ReactedKind {
    /// What a person typed.
    Person,
    /// An agent session's post, question, answer or note.
    Agent,
    /// An issue's card, from the forge (`x-zirc/forge`).
    Issue,
    /// Anything else: CI's lines, a deploy, a team's own event, a file.
    #[serde(other)]
    Other,
}

impl Reacted {
    /// How much of a message's text is kept.
    pub const TEXT: usize = 300;

    /// What `delivery` is, to a reaction on it.
    #[must_use]
    pub fn of(delivery: &Delivery) -> Self {
        use zirc_proto::Event;
        let session = Some(delivery.message.session.clone())
            .filter(|session| !session.is_empty() && session != zirc_proto::NO_SESSION);
        let (kind, text, link) = match &delivery.message.event {
            Event::Text { body } if session.is_none() => (ReactedKind::Person, body.clone(), None),
            Event::Text { body } => (ReactedKind::Agent, body.clone(), None),
            Event::Query { text, .. } | Event::Answer { text, .. } | Event::Note { text, .. } => {
                (ReactedKind::Agent, text.clone(), None)
            }
            Event::Custom { kind, data, text } => (
                if kind == FORGE {
                    ReactedKind::Issue
                } else {
                    ReactedKind::Other
                },
                text.clone(),
                first_link(data),
            ),
            event => (ReactedKind::Other, event.human_text(), None),
        };
        Self {
            from: delivery.from.clone(),
            session,
            kind,
            text: text.chars().take(Self::TEXT).collect(),
            link,
        }
    }
}

/// The kind of the forge's issue cards.
const FORGE: &str = "x-zirc/forge";

/// The first `https://` link in a card's data (`card.links`), if any.
fn first_link(data: &serde_json::Value) -> Option<String> {
    data.get(zirc_proto::card::KEY)
        .and_then(|card| card.pointer("/links/0/url"))
        .and_then(serde_json::Value::as_str)
        .filter(|url| url.starts_with("https://"))
        .map(str::to_owned)
}

/// A session on this machine as its owner knows it: what it's called, and where to open
/// it. Never sent to the chat server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LocalSession {
    /// The session's id.
    pub session: String,
    /// Its short handle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its full title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Where its owner opens it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// How often sessions' watches woke on one day for one reason, and how often the session
/// then acted within [`WAKE_ACTED_SECS`]: posted, committed, pushed, or asked zircd to do
/// something. Kept on the machine, never sent anywhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WakeCount {
    /// The day, `2026-11-05`, in UTC.
    pub day: String,
    /// Why the watches woke.
    pub why: Why,
    /// For [`Why::Intent`]: the intent was one zircd said for a session by itself, not one
    /// a session said.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub automatic: bool,
    /// How many times.
    pub woke: u64,
    /// How many of those the session acted on.
    pub acted: u64,
}

/// zircd's circuit breaker on wakes: how many it let through lately, its limits, and whether
/// it's holding them, past a limit, as notes for each session's next wake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BreakerState {
    /// The window wakes are counted over, in minutes.
    pub window_minutes: u64,
    /// The most wakes of one session in the window.
    pub per_session: usize,
    /// The most wakes of all this machine's sessions in the window.
    pub machine: usize,
    /// The wakes let through in the window.
    pub wakes: usize,
    /// Whether no session wakes, the machine's limit being reached.
    #[serde(default)]
    pub paused_machine: bool,
    /// The sessions that don't wake, each having reached its own limit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paused_sessions: Vec<String>,
    /// How many wakes were held since it last paused any.
    #[serde(default)]
    pub held: usize,
    /// What woke sessions most in the window, most first, as `mention from cc-1 ×5`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top: Vec<String>,
}

/// How soon after a wake a session's action counts as acting on it, in seconds.
pub const WAKE_ACTED_SECS: u64 = 15 * 60;

/// [`Request::Wakes`]'s days, unless said.
fn a_week() -> u32 {
    7
}

/// This machine's owner's answer to a request ([`zirc_proto::request`]): a reaction the
/// server said their own account gave, on the request's own message, while it was open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Answered {
    /// The request, as it now stands.
    pub request: DecisionRequest,
    /// The answer.
    pub answer: Answer,
    /// Who gave it: the owner's account.
    pub by: String,
    /// The reaction that gave it.
    pub emoji: String,
    /// When, in RFC 3339.
    pub at: String,
    /// The id of the request's message.
    pub msgid: String,
    /// The organisation whose server it came from, while watching on a machine signed in
    /// to several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    /// Why it concerns the watching session: the session asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<Why>,
}

/// A message sent to this machine's nick, as a person messages a machine on IRC. It can be
/// meant for one of the machine's sessions: one that starts with a session's name and a
/// colon, such as `cc-1531f8: rebase please`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Direct {
    /// The nick it came from.
    pub from: String,
    /// The sending session, when it came from another machine's zirc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_session: Option<String>,
    /// The session here it's meant for, if it names one of this machine's sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// What it says.
    pub text: String,
    /// When it's a query from another agent ([`zirc_proto::Event::Query`]): its id, which
    /// `zirc reply` answers it by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// When it answers a query this machine sent: that query's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// Whether it's a note from another agent session for the one it names
    /// ([`zirc_proto::Event::Note`]): a teammate's report, never the person's words.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub note: bool,
    /// Whether it's a reply from a member one of this machine's sessions wrote to
    /// (`zirc dm --to`), for that session: a colleague's word, never the person's, let in
    /// while the organisation allows it (`zirc_proto::colleagues`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub colleague: bool,
    /// The server's id for it (`msgid`), which [`Request::React`] names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Why it concerns the watching session, if it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<Why>,
    /// The organisation whose server it came from, while watching on a machine signed in
    /// to several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
}

/// What a client says when zircd doesn't know what it asked: zircd runs an older release than
/// the client, so restarting it starts the newer one.
#[must_use]
pub fn older_zircd() -> String {
    format!(
        "this zircd is older than this {PROGRAM}: restart it on the newer one ({PROGRAM} daemon restart)"
    )
}

/// Why talking to zircd failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The socket couldn't be reached or the connection broke, usually because zircd isn't
    /// running.
    #[error("can't reach zircd: {0}")]
    Io(std::io::Error),
    /// A line wasn't the JSON expected.
    #[error("unreadable message: {0}")]
    Json(#[from] serde_json::Error),
    /// zircd hung up without answering.
    #[error("zircd closed the connection without answering")]
    Closed,
    /// The request was written for a newer protocol than this zircd speaks.
    #[error(
        "the request is for socket protocol {0}, and this zircd speaks {PROTOCOL}: restart \
         zircd on the newer {PROGRAM} ({PROGRAM} daemon restart)"
    )]
    Protocol(u32),
    /// zircd answered with an error.
    #[error("{0}")]
    Daemon(String),
    /// A request's line ran past [`MAX_REQUEST_BYTES`] without ending.
    #[error("the request is longer than {MAX_REQUEST_BYTES} bytes")]
    TooLong,
}

/// The longest line [`receive_request`] reads. A post's text, a team event's data and every
/// other request are far shorter (a message reaches IRC in lines of a few KiB), so a longer
/// line is a client gone wrong, not one to hold in memory while it goes on.
pub const MAX_REQUEST_BYTES: u64 = 4 * 1024 * 1024;

// Not `#[from]`, which would make it the source too: then `{:#}` says it twice.
impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// Sends `request` to the zircd listening on `socket` and returns its answer.
///
/// # Errors
///
/// When zircd can't be reached, or answers with [`Response::Error`].
pub async fn request(socket: &Path, request: &Request) -> Result<Response, Error> {
    request_in(socket, None, request).await
}

/// Sends `request` for the organisation `org` to the zircd listening on `socket` and
/// returns its answer: without an organisation, as [`request`].
///
/// # Errors
///
/// When zircd can't be reached, or answers with [`Response::Error`].
pub async fn request_in(
    socket: &Path,
    org: Option<&str>,
    request: &Request,
) -> Result<Response, Error> {
    let (mut lines, mut write) = open(socket).await?;
    send(&mut write, &Versioned::new(request, org)).await?;
    match receive(&mut lines).await? {
        Some(Response::Error { message }) => Err(Error::Daemon(message)),
        Some(response) => Ok(response),
        None => Err(Error::Closed),
    }
}

/// Something heard while watching.
#[expect(
    clippy::large_enum_variant,
    reason = "each is handled as it's heard, not kept"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Heard {
    /// A message someone sent.
    Message(Delivery),
    /// A message sent to this machine's nick.
    Direct(Direct),
    /// A reaction to a message, or one taken back.
    Reaction(Reaction),
    /// This machine's owner answered a session's request.
    Answered(Box<Answered>),
    /// Someone else started changing paths this machine has changed too.
    Overlap(Overlap),
    /// Two worktrees on this machine started changing the same paths.
    LocalOverlap(LocalOverlap),
    /// A push changes the same lines as a worktree on this machine.
    Conflict(Conflict),
}

/// A stream of what's heard, from [`watch`].
pub struct Watching {
    lines: BufReader<OwnedReadHalf>,
    // Kept so the daemon sees the connection as open.
    _write: tokio::net::unix::OwnedWriteHalf,
}

impl Watching {
    /// The next thing heard, or `None` when zircd stops.
    ///
    /// # Errors
    ///
    /// When the connection breaks or zircd sends something it shouldn't while watching.
    ///
    /// A line this zirc can't read, as a newer zircd may send, is skipped with a note on
    /// stderr rather than ending the watch: a watch that ends wakes its session for nothing.
    pub async fn next(&mut self) -> Result<Option<Heard>, Error> {
        loop {
            match self.next_line().await {
                Err(Error::Json(err)) => {
                    eprintln!("zirc: skipped a line this zirc can't read ({err}): zirc update");
                }
                Err(Error::Daemon(message)) if message.starts_with(UNEXPECTED) => {
                    eprintln!("zirc: skipped {message}: zirc update");
                }
                heard => return heard,
            }
        }
    }

    async fn next_line(&mut self) -> Result<Option<Heard>, Error> {
        match receive(&mut self.lines).await? {
            Some(Response::Delivery(delivery)) => Ok(Some(Heard::Message(delivery))),
            Some(Response::Direct(direct)) => Ok(Some(Heard::Direct(direct))),
            Some(Response::Reaction(reaction)) => Ok(Some(Heard::Reaction(reaction))),
            Some(Response::Answered(answered)) => Ok(Some(Heard::Answered(answered))),
            Some(Response::Overlap(overlap)) => Ok(Some(Heard::Overlap(overlap))),
            Some(Response::LocalOverlap(overlap)) => Ok(Some(Heard::LocalOverlap(overlap))),
            Some(Response::Conflict(conflict)) => Ok(Some(Heard::Conflict(conflict))),
            Some(Response::Error { message }) => Err(Error::Daemon(message)),
            Some(other) => Err(Error::Daemon(format!("{UNEXPECTED}{other:?}"))),
            None => Ok(None),
        }
    }
}

/// How a watch says it heard an answer it doesn't watch for.
const UNEXPECTED: &str = "unexpected answer: ";

/// Starts watching zirc messages and overlaps through the zircd listening on `socket`, as
/// `session`: what's heard says why it concerns that session, if it does.
///
/// # Errors
///
/// When zircd can't be reached.
pub async fn watch(socket: &Path, session: Option<&str>) -> Result<Watching, Error> {
    let (lines, mut write) = open(socket).await?;
    let session = session.map(ToOwned::to_owned);
    send(
        &mut write,
        &Versioned::new(&Request::Watch { session }, None),
    )
    .await?;
    Ok(Watching {
        lines,
        _write: write,
    })
}

async fn open(
    socket: &Path,
) -> Result<(BufReader<OwnedReadHalf>, tokio::net::unix::OwnedWriteHalf), Error> {
    crate::paths::check_socket(socket)?;
    let (read, write) = UnixStream::connect(socket).await?.into_split();
    Ok((BufReader::new(read), write))
}

/// Writes `value` as one line of JSON.
///
/// # Errors
///
/// When writing fails.
pub async fn send<W, T>(write: &mut W, value: &T) -> Result<(), Error>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    write.write_all(&line).await?;
    write.flush().await?;
    Ok(())
}

/// A request with the protocol version it's written for, and the organisation it's for.
#[derive(Serialize, Deserialize)]
struct Versioned<R> {
    #[serde(default = "first")]
    v: u32,
    // Not `org`, which `join` and `leave` have already.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    organisation: Option<String>,
    #[serde(flatten)]
    request: R,
}

impl<'a> Versioned<&'a Request> {
    fn new(request: &'a Request, org: Option<&str>) -> Self {
        Self {
            v: PROTOCOL,
            organisation: org.map(ToOwned::to_owned),
            request,
        }
    }
}

/// A request as zircd reads it: what's asked, and of which organisation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addressed {
    /// What's asked.
    pub request: Request,
    /// The organisation it's for, when the client said: `"organisation"` beside `"op"`. Without
    /// one, it's for the organisation of the repository it names, or the machine's only
    /// one.
    pub org: Option<String>,
}

fn first() -> u32 {
    1
}

/// Reads one request, as zircd does, or `None` at the end of the stream.
///
/// # Errors
///
/// When reading fails, the line is longer than [`MAX_REQUEST_BYTES`] or isn't a request, or
/// it's for a newer protocol: see [`PROTOCOL`].
pub async fn receive_request<R>(lines: &mut R) -> Result<Option<Addressed>, Error>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    let read = (&mut *lines)
        .take(MAX_REQUEST_BYTES + 1)
        .read_until(b'\n', &mut line)
        .await?;
    if read == 0 {
        return Ok(None);
    }
    if line.len() as u64 > MAX_REQUEST_BYTES {
        return Err(Error::TooLong);
    }
    let versioned: Versioned<Request> = serde_json::from_slice(&line)?;
    if versioned.v > PROTOCOL {
        return Err(Error::Protocol(versioned.v));
    }
    Ok(Some(Addressed {
        request: versioned.request,
        org: versioned.organisation,
    }))
}

/// Reads one line of JSON, or `None` at the end of the stream.
///
/// # Errors
///
/// When reading fails or the line isn't a `T`.
pub async fn receive<R, T>(lines: &mut R) -> Result<Option<T>, Error>
where
    R: AsyncBufRead + Unpin,
    T: DeserializeOwned,
{
    let mut line = String::new();
    if lines.read_line(&mut line).await? == 0 {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&line)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_read_as_json_a_person_could_write() {
        let request: Request =
            serde_json::from_str(r#"{"op":"repo-add","path":"/src/api"}"#).unwrap();
        assert_eq!(
            request,
            Request::RepoAdd {
                path: "/src/api".into(),
                session: None,
            }
        );
        assert_eq!(
            serde_json::to_string(&Request::Status).unwrap(),
            r#"{"op":"status"}"#
        );
    }

    #[tokio::test]
    async fn sends_and_receives_lines() {
        let (client, server) = tokio::io::duplex(1024);
        let (_, mut write) = tokio::io::split(client);
        let (read, _) = tokio::io::split(server);
        let mut lines = BufReader::new(read);

        send(&mut write, &Response::Done).await.unwrap();
        let watch = Request::Watch {
            session: Some("cc-1".into()),
        };
        send(&mut write, &watch).await.unwrap();
        drop(write);
        assert_eq!(receive(&mut lines).await.unwrap(), Some(Response::Done));
        assert_eq!(receive(&mut lines).await.unwrap(), Some(watch));
        assert_eq!(receive::<_, Request>(&mut lines).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_request_longer_than_the_cap_is_refused_unread() {
        let long = usize::try_from(MAX_REQUEST_BYTES).unwrap() + 10;
        let mut endless = BufReader::new(std::io::Cursor::new(vec![b' '; long]));
        assert!(matches!(
            receive_request(&mut endless).await,
            Err(Error::TooLong)
        ));
        let status = format!("{}\n", serde_json::to_string(&Request::Status).unwrap());
        let padded = format!("{}{status}", " ".repeat(64 * 1024));
        let mut lines = BufReader::new(std::io::Cursor::new(padded));
        assert_eq!(
            receive_request(&mut lines).await.unwrap(),
            Some(Addressed {
                request: Request::Status,
                org: None,
            }),
            "a long request under the cap is read"
        );
    }

    #[tokio::test]
    async fn requests_say_their_protocol_and_newer_ones_are_refused() {
        let line = |json: &str| BufReader::new(std::io::Cursor::new(format!("{json}\n")));
        let read = |request| Some(Addressed { request, org: None });
        let sent = serde_json::to_string(&Versioned::new(&Request::Status, None)).unwrap();
        assert_eq!(sent, format!(r#"{{"v":{PROTOCOL},"op":"status"}}"#));
        assert_eq!(
            receive_request(&mut line(&sent)).await.unwrap(),
            read(Request::Status)
        );
        // From a client that predates versions.
        assert_eq!(
            receive_request(&mut line(r#"{"op":"who"}"#)).await.unwrap(),
            read(Request::Who)
        );
        // For one organisation of several.
        let sent = serde_json::to_string(&Versioned::new(&Request::Who, Some("acme"))).unwrap();
        assert_eq!(
            sent,
            format!(r#"{{"v":{PROTOCOL},"organisation":"acme","op":"who"}}"#)
        );
        assert_eq!(
            receive_request(&mut line(&sent)).await.unwrap(),
            Some(Addressed {
                request: Request::Who,
                org: Some("acme".into()),
            })
        );
        // Beside a request with an `org` of its own.
        let join = Request::Join {
            repo: "/src/api".into(),
            topic: "auth".into(),
            org: true,
            session: None,
        };
        let sent = serde_json::to_string(&Versioned::new(&join, Some("acme"))).unwrap();
        assert_eq!(
            receive_request(&mut line(&sent)).await.unwrap(),
            Some(Addressed {
                request: join,
                org: Some("acme".into()),
            })
        );
        let newer = format!(r#"{{"v":{},"op":"who"}}"#, PROTOCOL + 1);
        assert!(matches!(
            receive_request(&mut line(&newer)).await,
            Err(Error::Protocol(v)) if v == PROTOCOL + 1
        ));
    }

    #[test]
    fn an_older_watch_is_the_machines() {
        let older: Request = serde_json::from_str(r#"{"op":"watch"}"#).unwrap();
        assert_eq!(older, Request::Watch { session: None });
    }
}
