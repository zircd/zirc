//! The daemon's state, owned by one task. Everything that happens reaches it as an
//! [`Input`] — socket requests, IRC lines, file-system touches — and it handles them one at
//! a time, so nothing else needs a lock.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use zirc_proto::PROGRAM;

use anyhow::{Context as _, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use tokio::sync::{mpsc, oneshot};
use zirc_core::agents::Agents;
use zirc_core::decisions;
use zirc_core::history::{self, Pages};
use zirc_core::ipc::{
    Answered, Delivery, Direct, Reaction, RepoSession, Request, Response, Why, Worktree,
};
use zirc_core::paths;
use zirc_core::presence::{Conflict, LocalOverlap, Overlap, Presence};
use zirc_core::project::Project;
use zirc_irc::{Client, Line};
use zirc_proto::decision::Decision;
use zirc_proto::{Event, Level, Message};

use crate::git::{self, Change, Side, Snapshot};

mod announce;
mod asking;
mod auto_intents;
mod awaits;
mod batches;
pub(crate) mod book;
pub(crate) mod breaker;
mod broadcast;
mod busy;
mod deliver;
mod fit;
mod gotchas;
mod holds;
mod incoming;
mod intents_told;
mod later;
mod lease;
mod live;
mod moved;
mod ops_kept;
mod post_room;
mod queries;
mod replay;
mod repos;
mod requests;
mod respond;
mod say;
mod scans;
mod sealing;
mod sessions;
mod tasks;
mod unowned;
mod wakes;
mod warn_once;
mod worktree_updates;

/// A repository is scanned once its files have been quiet this long...
const QUIET: Duration = Duration::from_millis(500);
/// ...or once it's been touched this long ago, however busy it still is.
const MAX_WAIT: Duration = Duration::from_secs(3);
/// How often touched repositories are checked against the two limits above.
const SCAN_INTERVAL: Duration = Duration::from_millis(200);
/// How often watched main checkouts are looked at for new worktrees to watch
/// ([`Daemon::find_worktrees`]): a directory listing each, without Git.
const WORKTREES_EVERY: Duration = Duration::from_secs(2);
/// The session events from the file watcher are sent as.
const WATCHER_SESSION: &str = zirc_proto::NO_SESSION;

/// Something for the daemon to handle.
pub enum Input {
    /// A request from a socket client, and where to send the answer.
    Request(Request, oneshot::Sender<Response>),
    /// A socket client that wants every delivery and overlap from now on, watching as the
    /// session named, if any.
    Watch(mpsc::Sender<Response>, Option<String>),
    /// The IRC connection is up.
    Connected(Client),
    /// The IRC connection went down.
    Disconnected,
    /// The web app no longer takes this machine's account, so it's stopped connecting.
    Revoked,
    /// It's stopped connecting for this reason, until `config.toml` changes: as a guest's
    /// terminal the web app lets in no more, which couldn't be taken off the machine.
    Refused(String),
    /// The binaries were updated to this release, so it's time to restart.
    Updated(zirc_core::update::Version),
    /// A line from the IRC server.
    Line(Line),
    /// Something changed in the watched repository with this root.
    Touched(PathBuf),
    /// What reading the repository with this root found.
    Scanned(PathBuf, Result<scans::Scan>),
    /// What a push was found to clash with on this machine ([`Daemon::check_push`]).
    PushChecked(scans::Clashes),
    /// The overlaps between this machine's worktrees in this channel, with the lines Git
    /// says they share ([`Daemon::check_overlaps`]).
    Predicted(String, Vec<LocalOverlap>),
    /// Of this machine's worktrees and other machines' sessions, the lines both change, as
    /// Git says from the lines those sessions said ([`Daemon::predict_remote`]).
    RemotePredicted(Vec<(fit::RemoteKey, fit::Predicted)>),
    /// The request waiting on the server with this token has waited long enough.
    Expired(u64),
    /// Whether this daemon watches the repository with this root, or will once its restore
    /// gets to it: the router's question for which organisation a request is for, answered
    /// even while restoring, so it never waits for one.
    Holds(PathBuf, oneshot::Sender<bool>),
}

/// A request waiting on the server, answered once the server says, or when it's taken too
/// long.
struct Asking {
    token: u64,
    answer: oneshot::Sender<Response>,
    what: Ask,
}

/// What a request waits on the server for.
enum Ask {
    /// Whether message `id` was trimmed from `target`.
    Trim { target: String, id: String },
    /// Whether the server has channel `target` (`LIST`), before joining it to read its
    /// history as `pages` says: joining a channel the server hasn't makes it, and with
    /// persistent history an empty one stays, in every app's list.
    Listing {
        target: String,
        pages: Pages,
        found: bool,
    },
    /// `target`'s history, read back a page at a time as `pages` says: the page in the batch
    /// named `batch` once it starts, every item of it `seen` (joins, parts and quits too,
    /// which page as messages do) and its messages each by its place there, and the
    /// `messages` kept from the pages before.
    Read {
        target: String,
        batch: Option<String>,
        seen: Vec<history::Seen>,
        page: Vec<(usize, Delivery)>,
        messages: Vec<Delivery>,
        pages: Pages,
    },
    /// The echo of what `session` just posted in `channel`, whose id makes the post's link.
    Echo { channel: String, session: String },
    /// The private conversations with messages lately, in the batch named `batch` once it
    /// starts.
    Conversations {
        batch: Option<String>,
        targets: Vec<String>,
    },
    /// The message `reaction` is on, which zircd hadn't heard (`CHATHISTORY AROUND`), in the
    /// batch named `batch` once it starts: `found` once it's in it. The reaction is told
    /// once it's all in ([`Daemon::fetched`]).
    Reacted {
        reaction: Box<Reaction>,
        batch: Option<String>,
        found: Option<Box<Delivery>>,
    },
    /// Whether the server's workflows' host is on (`ISON`), before `session` sends it a
    /// workflow's file, `event` ([`Daemon::deploy`]).
    Hosted { session: String, event: Box<Event> },
}

impl Ask {
    /// Waiting for `target`'s history, as `pages` says.
    fn reading(target: String, pages: Pages) -> Self {
        Self::Read {
            target,
            batch: None,
            seen: Vec::new(),
            page: Vec::new(),
            messages: Vec::new(),
            pages,
        }
    }

    fn target(&self) -> &str {
        match self {
            Self::Trim { target, .. }
            | Self::Read { target, .. }
            | Self::Listing { target, .. } => target,
            Self::Reacted { reaction, .. } => &reaction.channel,
            Self::Conversations { .. } | Self::Echo { .. } | Self::Hosted { .. } => "",
        }
    }
}

struct Watched {
    /// Its common Git directory, the same for every worktree of one repository
    /// ([`git::common_dir`]).
    common: PathBuf,
    project: Project,
    /// The repository's channel, `#<repo>`.
    channel: String,
    /// Its agent room, where its sessions' lines go ([`post_room`]): `#<repo>/agents` unless the
    /// team's config says otherwise, `None` when that's the repository's channel.
    room: Option<String>,
    /// Its areas' rooms, `#<repo>/<area>`, as the team's config names them ([`post_room`]).
    areas: Vec<String>,
    default_branch: String,
    snapshot: Snapshot,
    /// When the repository was first and last touched since its last scan.
    touched: Option<(Instant, Instant)>,
    /// Whether the last scan was held, as a worktree read while it was being removed
    /// ([`git::half_removed`]): the next is believed.
    held: bool,
    /// The sessions working here, which sign their commits and pushes.
    sessions: Sessions,
    /// The subjects of the commits announced lately, so a rebase or an amend that keeps a
    /// commit's subject doesn't announce it again.
    announced: VecDeque<String>,
    _watcher: RecommendedWatcher,
}

/// A part number as long as any a working set will need, to leave room for it while the
/// set is cut into parts that fit a line.
const PART_ROOM: u32 = 9999;

/// How many commit subjects each repository remembers having announced.
const ANNOUNCED: usize = 64;

/// A watched repository as saved across restarts, or a linked worktree a person stopped
/// watching, which isn't watched again by itself ([`Daemon::unwatched`]). One without an
/// organisation, as before several, is the first's.
#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    path: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sessions: Vec<RepoSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    org: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    unwatched: bool,
}

impl Saved {
    /// Whether it's `org`'s: saves without an organisation are the first's.
    fn is_of(&self, org: &Org) -> bool {
        self.org
            .as_ref()
            .map_or(org.first, |name| *name == org.name)
    }
}

/// An entry of `repos.json`: a [`Saved`], or something else, such as the bare path a zircd
/// from before 1 Nov 2026 saved, which is dropped rather than make the file unreadable.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum SavedEntry {
    Saved(Saved),
    Other(serde_json::Value),
}

/// The watched repositories saved at `path` ([`read_saved`]), every organisation's.
fn read_saved_repos(path: &Path) -> Option<Vec<Saved>> {
    let entries = read_saved::<Vec<SavedEntry>>(path)?;
    Some(
        entries
            .into_iter()
            .filter_map(|entry| match entry {
                SavedEntry::Saved(saved) => Some(saved),
                SavedEntry::Other(other) => {
                    tracing::warn!("dropping {other} from the watched repositories: unreadable");
                    None
                }
            })
            .collect(),
    )
}

/// `repos.json` holds every organisation's repositories, each organisation's daemon saving
/// its own: one saves at a time, so none saves over another's.
static SAVING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The organisation a daemon is the connection to. Nothing about a repository is ever sent
/// to another organisation's server, so each daemon watches its organisation's repositories
/// alone.
#[derive(Debug, Clone)]
pub struct Org {
    /// Its name.
    pub name: String,
    /// Whether it's the first in `config.toml`, which repositories saved before several
    /// organisations belong to.
    pub first: bool,
    /// Where its own things are kept, such as its topics: `home` for the first.
    pub state: PathBuf,
    /// Whether the machine is signed in to other organisations too. Only then do its
    /// repositories say whose they are: someone in one organisation sees no change.
    pub several: bool,
    /// When it's a guest's terminal rather than the machine's own: its guest's room.
    pub guest: Option<Guest>,
}

/// A guest's terminal ([`zirc_proto::guest_terminal`]): in its guest's room alone, where
/// its sessions' posts go and whatever anyone else says wakes them, as their conversation.
/// Nothing else is sent there, no repository, topic, intent, decision or state: the gateway
/// refuses all but the room.
#[derive(Debug, Clone)]
pub struct Guest {
    /// The guest's room, such as `#web/guest-21042`: the one channel it's in.
    pub room: String,
    /// The public channel it's a guest of, as `zirc login` names it, such as `acme/#web`.
    pub of: String,
}

impl Guest {
    /// Whether a guest's terminal answers `request` itself: what it posts and reacts in its
    /// room, and what's about this machine alone. The rest is refused, as the gateway would.
    fn takes(request: &Request) -> bool {
        matches!(
            request,
            Request::Status
                | Request::RepoList
                | Request::Post { .. }
                | Request::SealedPost { .. }
                | Request::Unseal { .. }
                | Request::React { .. }
                | Request::Typing { .. }
                | Request::Name { .. }
                | Request::Queries { .. }
                | Request::Who
                | Request::LocalSessions
                | Request::Wakes { .. }
                | Request::WakesResume
                | Request::WaitFor { .. }
                | Request::Stack { .. }
                | Request::Stop
                | Request::Restart
        )
    }
}

/// The sessions working in one checkout, the last one added last.
///
/// The last one added works on all of it, as one session at a time does.
#[derive(Debug, Clone, Default)]
struct Sessions(Vec<RepoSession>);

impl Sessions {
    /// Adds `session`, or makes it the last one added.
    fn add(&mut self, session: String) {
        if let Some(i) = self.0.iter().position(|it| it.session == session) {
            self.0.remove(i);
        }
        self.0.push(RepoSession { session });
    }

    /// The last session added.
    fn latest(&self) -> Option<&str> {
        self.0.last().map(|it| it.session.as_str())
    }

    fn contains(&self, session: &str) -> bool {
        self.0.iter().any(|it| it.session == session)
    }

    /// Takes `session` out, saying whether it was in.
    fn remove(&mut self, session: &str) -> bool {
        let before = self.0.len();
        self.0.retain(|it| it.session != session);
        self.0.len() < before
    }

    fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|it| it.session.as_str())
    }
}

/// What a post says to reach every session that hears its channel, whatever it's working
/// on or has joined.
const EVERYONE: &str = "@everyone";

/// Topic channels joined, each with who joined it.
type Topics = BTreeMap<String, BTreeSet<Option<String>>>;

/// The joined topics as saved across restarts. Older saves are a list of channels, joined
/// by the machine.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum SavedTopics {
    Channels(BTreeSet<String>),
    Joiners(Topics),
}

impl Default for SavedTopics {
    fn default() -> Self {
        Self::Joiners(Topics::new())
    }
}

impl From<SavedTopics> for Topics {
    fn from(saved: SavedTopics) -> Self {
        match saved {
            SavedTopics::Channels(channels) => channels
                .into_iter()
                .map(|channel| (channel, BTreeSet::from([None])))
                .collect(),
            SavedTopics::Joiners(topics) => topics,
        }
    }
}

/// One working set in a checkout: all of its changes.
struct Set<'a> {
    /// The branch its paths are changed on.
    branch: Option<String>,
    /// The session whose work it is, if known.
    session: Option<&'a str>,
    paths: BTreeSet<String>,
    /// What its work is built on ([`git::Snapshot::base`]).
    base: Option<String>,
    /// The paths its own commits change beyond `paths` ([`git::Snapshot::committed`]).
    committed: BTreeSet<String>,
    /// The lines of `base` its work changes ([`git::Snapshot::lines`]).
    lines: BTreeMap<String, Vec<[u32; 2]>>,
}

impl Watched {
    /// Adds `session` here, as [`Sessions::add`] does.
    fn add_session(&mut self, session: String) {
        self.sessions.add(session);
    }

    /// The working sets here: all the changes, the last session added's.
    fn sets(&self) -> Vec<Set<'_>> {
        vec![Set {
            branch: self.snapshot.branch.clone(),
            session: self.sessions.latest(),
            paths: self.snapshot.dirty.clone(),
            base: self.snapshot.base.clone(),
            committed: self.snapshot.committed.clone(),
            lines: self.snapshot.lines.clone(),
        }]
    }

    /// The paths `session` is working on here: all of them, for any session added with
    /// them.
    fn paths_of(&self, session: &str) -> BTreeSet<String> {
        if !self.sessions.contains(session) {
            return BTreeSet::new();
        }
        // A plain checkout's changes are its one session's; one that several share, as the
        // main checkout was, has nobody's: each of its sessions was woken by every intent,
        // push and commit touching any of them (6 Nov 2026).
        if self.sessions.names().count() == 1 {
            self.snapshot.dirty.clone()
        } else {
            BTreeSet::new()
        }
    }

    /// Whether `session` works here.
    fn is_of(&self, session: &str) -> bool {
        self.sessions.contains(session)
    }

    /// The session that signs what's made here: the last added.
    fn signer(&self) -> &str {
        self.sessions.latest().unwrap_or(WATCHER_SESSION)
    }

    /// The channel of the branch checked out here, unless that's the default one.
    ///
    /// Only a branch that's been pushed has one: an unpushed branch, such as the one a tool
    /// makes for each agent session, has only its own worktree on it, and a channel of its
    /// own would hold nobody else.
    fn branch_channel(&self) -> Option<String> {
        self.channel_of(self.snapshot.branch.as_deref()?)
    }

    /// The channel of `branch`, if it's been pushed and isn't the default one.
    fn channel_of(&self, branch: &str) -> Option<String> {
        let suffix = format!("/{branch}");
        if !self
            .snapshot
            .remotes
            .keys()
            .any(|remote| remote.ends_with(&suffix))
        {
            return None;
        }
        self.project.branch_channel(branch, &self.default_branch)
    }
}

/// Runs `daemon` on what it `received` until it stops, scanning its repositories as they
/// change.
pub async fn run(mut daemon: Daemon, mut received: mpsc::UnboundedReceiver<Input>) {
    let held = daemon.restore(&mut received).await;
    for input in held {
        daemon.handle(input).await;
    }
    let mut scans = tokio::time::interval(SCAN_INTERVAL);
    let mut worktrees = tokio::time::interval(WORKTREES_EVERY);
    let mut leases = tokio::time::interval(Duration::from_secs(1));
    while !daemon.stopping {
        // Measured by the real clock, which a test's doesn't stand in for: how long it took.
        let start;
        tokio::select! {
            Some(input) = received.recv() => {
                start = Instant::now();
                daemon.handle(input).await;
            }
            _ = scans.tick() => {
                start = Instant::now();
                daemon.scan();
            }
            _ = worktrees.tick() => {
                start = Instant::now();
                daemon.find_worktrees().await;
            }
            _ = leases.tick() => {
                start = Instant::now();
                daemon.lease_tick().await;
                daemon.requests_tick();
                daemon.wakes_tick();
                daemon.unowned_tick();
                daemon.holds_tick();
                daemon.replay_tick();
                daemon.live_tick(now());
                daemon.forget_sessions(now());
            }
        }
        daemon.busy.took(start, Instant::now());
    }
    daemon.save_replay();
    daemon.save_live(now());
    if daemon.restarting {
        daemon.finish_scans(&mut received, SCANS_FINISH).await;
        // Answered rather than dropped: what's waiting gets the old zircd's answer.
        while let Ok(input) = received.try_recv() {
            if let Input::Request(..) = input {
                daemon.handle(input).await;
            }
        }
    }
}

/// How long `zirc` warns that a session was registered by two worktrees ([`Daemon::shared_with`]).
const SHARED_FOR: u64 = 86_400;

/// How long a restarting zircd waits for its scans in flight to end.
const SCANS_FINISH: Duration = Duration::from_secs(10);

/// How many saved repositories a restore reads at once.
const RESTORE_AT_ONCE: usize = 4;

pub struct Daemon {
    home: PathBuf,
    /// `config.toml`, read again when it changes ([`crate::config::File`]).
    config: crate::config::File,
    org: Org,
    server: String,
    /// The handle of the person this machine belongs to, which posts can name too.
    owner: Option<String>,
    /// Which of others' intents each session has been woken for.
    intents_told: intents_told::IntentsTold,
    /// What each channel was last told of this machine's working sets, by worktree.
    worktree_updates: worktree_updates::WorktreeUpdates,
    inputs: mpsc::UnboundedSender<Input>,
    client: Option<Client>,
    joined: BTreeSet<String>,
    /// The rooms of a repository this machine has just started watching, whose owner is
    /// invited to each once this machine is in it, so their apps join it too.
    invite_owner: BTreeSet<String>,
    /// `#ops`'s newest messages, kept to answer reads of states at once.
    ops: Option<ops_kept::Kept>,
    repos: BTreeMap<PathBuf, Watched>,
    /// Topic channels joined by choice, each with who joined it: sessions by name, and
    /// `None` for the machine, as a person or an older zircd joins.
    topics: Topics,
    presence: Presence,
    watchers: Vec<Watcher>,
    /// How long its one task was busy with one thing lately ([`busy`]).
    busy: busy::Busy,
    /// The repositories being read now.
    scanning: BTreeSet<PathBuf>,
    /// How many checks of a push or of overlaps Git is being asked about now, each in a
    /// task of its own.
    checking: usize,
    /// Linked worktrees a person stopped watching with `zirc repo remove`, which aren't
    /// watched again by themselves as their main checkout's are ([`Daemon::find_worktrees`]),
    /// only by `zirc repo add`.
    unwatched: BTreeSet<PathBuf>,
    /// The warnings about repositories said already, not to be said at every scan.
    warned: warn_once::WarnOnce,
    /// When to try joining the channels it should be in and isn't, again.
    rejoin_at: Instant,
    /// The messages heard lately, to react to.
    recent: Recent,
    /// The sessions typing, each to whom, until when and when it was last said.
    typing: BTreeMap<(String, String), Typing>,
    /// When each channel was last told what this machine is working on, and what it was
    /// told; and the channels to tell once they're due.
    last_announced: BTreeMap<String, (Instant, Vec<(String, Event)>)>,
    to_announce: BTreeSet<String>,
    /// How often at most a channel is told what this machine is working on.
    pub announce_every: Duration,
    /// What concerned each session that has watched while none of its watches was open.
    missed: BTreeMap<String, VecDeque<(Instant, Response)>>,
    /// What was said while zircd wasn't there, asked for as it joins again.
    replay: replay::Replay,
    /// The overlaps told lately, so one that comes again unchanged isn't told again.
    told: Told,
    /// What Git last said of the lines worktrees here both change, for intents' fits.
    predictions: fit::Predictions,
    /// The ids of the messages heard lately.
    heard: Heard,
    /// The server's history batches open now: their lines are never news.
    batches: batches::Batches,
    /// Why it isn't connecting, when it's stopped trying until it's set up again.
    problem: Option<String>,
    /// Requests waiting on the server, and the token the next one gets.
    asking: Vec<Asking>,
    next_token: u64,
    /// How long a request waits on the server before it fails: [`zirc_core::trim::WAIT`].
    pub ask_wait: Duration,
    /// Set once a client has asked the daemon to stop, or it's restarting.
    stopping: bool,
    /// Set when it's to start again from its binary, as after an update.
    restarting: bool,
    /// Whether this machine logged in through zirc.dev, which makes the server's accounts:
    /// then an account `<owner>-…` is surely one of the owner's machines.
    zirc_dev: bool,
    /// The account this machine logs in as, which its own posts are sent from.
    account: Option<String>,
    /// Who may query this machine's agents ([`agents`]).
    agents: Agents,
    /// The queries this machine's sessions asked lately, to take only their answers.
    asked: VecDeque<Asked>,
    /// The queries heard lately, to send their answers back.
    queried: VecDeque<Queried>,
    /// Counts queries asked, to name each apart.
    queries_asked: u64,
    /// Leases this machine's sessions hold or wait for, and what it's heard of others'.
    leases: lease::LeaseState,
    /// The requests this machine's sessions asked its owner that are still open.
    requests: requests::Requests,
    /// The intents said for sessions automatically, and the ones they said themselves.
    auto_intents: auto_intents::AutoIntents,
    /// How sessions' watches woke, and whether they acted on it.
    wakes: wakes::Wakes,
    /// When each sender last woke every session with `@everyone` ([`broadcast`]).
    broadcasts: broadcast::Broadcasts,
    /// What each session waits on ([`awaits`]).
    awaits: awaits::Awaits,
    /// The gotchas heard in the repositories' gotchas rooms ([`gotchas`]).
    gotchas: gotchas::Kept,
    /// Which gotchas each session was told.
    gotchas_told: gotchas::Told,
    /// Each session's last post and when, so the same post again at once isn't said twice
    /// ([`Daemon::post`]).
    last_posts: HashMap<String, (String, u64)>,
    /// Breakages nobody owns, told to the machine's owner once they've gone too long
    /// ([`unowned`]).
    unowned: unowned::Unowned,
    /// The states this machine said that end by themselves ([`holds`]).
    holds: holds::Holds,
    /// The reports waiting for each session's next wake ([`later`]).
    later: later::Later,
    /// What each session has lately heard in a digest of [`later`], so the next leaves it out.
    later_told: later::Told,
    /// What sessions are told of the default branch at their next wake ([`moved`]).
    moved: moved::Moved,
    /// The circuit breaker on wakes.
    pub(crate) breaker: breaker::Breaker,
    /// The clock the breaker's window and a gone watch's leases are counted by.
    pub clock: crate::clock::Clock,
    /// How long a yes to an irreversible request is held before it counts.
    pub request_hold: Duration,
    /// Folded nick → the account the server last said it's logged in as (`account-tag`),
    /// for those heard: a nick without one can be anyone's, on a server that lets people
    /// connect without an account.
    accounts: HashMap<String, String>,
    /// Folded nick → whose machine it is, its owner's handle, as its host last said
    /// (`machine/<owner>`, `agents::host_owner`): a nick heard without one is a person's.
    machines: HashMap<String, String>,
    /// What this machine's sessions go by ([`book`]): one book, every organisation's daemon's.
    pub book: book::Shared,
    /// Session → the name and title its card last said in this organisation ([`sessions`]),
    /// to say it again when either changes.
    carded: BTreeMap<String, sessions::Carded>,
    /// When this daemon last forgot the sessions the book no longer knows ([`sessions`]).
    forgot_at: u64,
    /// The task each session is on, and finished last ([`tasks`]).
    tasks: tasks::Tasks,
    /// Session → the channel its `taking:` went to, for zircd's `done:` when its landing
    /// lands ([`tasks`]). Not kept across restarts, where the session says `done:` itself.
    tasks_taken_in: BTreeMap<String, String>,
    /// Session → the task its queued landing claimed while on it, which that landing
    /// landing ends ([`tasks`]). Not kept across restarts either.
    tasks_landing: BTreeMap<String, String>,
    /// By root, the sessions registered there in the last day that another worktree, still
    /// there, had registered ([`zirc_core::ipc::Shared`]).
    shared: BTreeMap<PathBuf, Vec<zirc_core::ipc::Shared>>,
    /// The repositories saved from before it last stopped, as saved, that a restore hasn't
    /// watched yet: still this daemon's ([`Input::Holds`]).
    restoring: BTreeSet<PathBuf>,
    /// Which of this machine's sessions are live, and where that was said ([`live`]).
    live: live::Live,
    /// Other machines' live sessions, and every session seen live ([`live::Elsewhere`]).
    elsewhere: live::Elsewhere,
}

/// A query a session here asked.
#[derive(Debug)]
struct Asked {
    id: String,
    /// The session that asked.
    session: String,
    /// The machines it went to, whose answers are taken.
    machines: Vec<String>,
}

/// A query heard, to answer.
#[derive(Debug)]
struct Queried {
    id: String,
    /// The machine that asked: this one, for its own sessions' queries.
    from: String,
    /// The session that asked.
    session: String,
}

/// How many queries asked and heard zircd remembers, to match answers to them.
const QUERIES: usize = 200;

impl Daemon {
    pub fn new(
        home: PathBuf,
        org: Org,
        server: String,
        owner: Option<String>,
        inputs: mpsc::UnboundedSender<Input>,
    ) -> Self {
        let agents = Agents::load(&paths::agents(&home));
        let config = crate::config::File::at(&paths::config(&home));
        let zirc_dev = config
            .ask(|settings| Some(crate::config::zirc_dev(settings, &org.name)))
            .unwrap_or(false);
        let account = config.ask(|settings| {
            let path = paths::config(&home);
            let connection = crate::config::load_in(settings, &path, &org.name, false).ok()?;
            Some(connection.sasl?.account)
        });
        let book = book::Shared::load(&home, now());
        let carded = sessions::carded(&book);
        let tasks = tasks::Tasks::load(&org.state);
        let live = live::Live::read(&org.state);
        Self {
            config,
            home,
            org,
            server,
            owner,
            intents_told: intents_told::IntentsTold::default(),
            worktree_updates: worktree_updates::WorktreeUpdates::default(),
            inputs,
            client: None,
            joined: BTreeSet::new(),
            invite_owner: BTreeSet::new(),
            ops: None,
            repos: BTreeMap::new(),
            topics: Topics::new(),
            presence: Presence::default(),
            watchers: Vec::new(),
            busy: busy::Busy::new(Instant::now()),
            scanning: BTreeSet::new(),
            checking: 0,
            unwatched: BTreeSet::new(),
            warned: warn_once::WarnOnce::default(),
            rejoin_at: Instant::now() + REJOIN_EVERY,
            recent: Recent::default(),
            typing: BTreeMap::new(),
            last_announced: BTreeMap::new(),
            announce_every: Duration::from_secs(60),
            to_announce: BTreeSet::new(),
            missed: BTreeMap::new(),
            replay: replay::Replay::default(),
            told: Told::default(),
            predictions: fit::Predictions::default(),
            heard: Heard::default(),
            batches: batches::Batches::default(),
            problem: None,
            asking: Vec::new(),
            next_token: 0,
            ask_wait: zirc_core::trim::WAIT,
            stopping: false,
            restarting: false,
            zirc_dev,
            account,
            agents,
            asked: VecDeque::new(),
            queried: VecDeque::new(),
            queries_asked: 0,
            leases: lease::LeaseState::default(),
            requests: requests::Requests::default(),
            auto_intents: auto_intents::AutoIntents::default(),
            wakes: wakes::Wakes::default(),
            broadcasts: broadcast::Broadcasts::default(),
            awaits: awaits::Awaits::default(),
            gotchas: gotchas::Kept::default(),
            gotchas_told: gotchas::Told::default(),
            last_posts: HashMap::new(),
            unowned: unowned::Unowned::default(),
            holds: holds::Holds::default(),
            elsewhere: live::Elsewhere::default(),
            later: later::Later::default(),
            later_told: later::Told::default(),
            moved: moved::Moved::default(),
            breaker: breaker::Breaker::default(),
            clock: crate::clock::Clock::default(),
            request_hold: requests::HOLD,
            accounts: HashMap::new(),
            machines: HashMap::new(),
            book,
            carded,
            forgot_at: 0,
            tasks,
            tasks_taken_in: BTreeMap::new(),
            tasks_landing: BTreeMap::new(),
            shared: BTreeMap::new(),
            restoring: BTreeSet::new(),
            live,
        }
    }

    /// Watches the repositories it watched before it last stopped, and rejoins its topics.
    /// Reading many repositories takes a while, so between them it connects and says how
    /// it is as soon as it can, which `zirc login` waits on; what else is `received`
    /// meanwhile is held, in order, for once it's done.
    pub async fn restore(&mut self, received: &mut mpsc::UnboundedReceiver<Input>) -> Vec<Input> {
        let mut held = Vec::new();
        self.restore_replay();
        let topics = read_saved::<SavedTopics>(&paths::topics(&self.org.state));
        let saved = read_saved_repos(&paths::repos(&self.home));
        let readable = topics.is_some() && saved.is_some();
        self.topics = topics.unwrap_or_default().into();
        let (unwatched, saved): (Vec<Saved>, Vec<Saved>) = saved
            .unwrap_or_default()
            .into_iter()
            .filter(|saved| self.org.guest.is_none() && saved.is_of(&self.org))
            .partition(|saved| saved.unwatched);
        self.unwatched = unwatched.into_iter().map(|saved| saved.path).collect();
        let saved: Vec<(PathBuf, Vec<RepoSession>)> = saved
            .into_iter()
            .map(|saved| (saved.path, saved.sessions))
            .collect();
        self.restoring = saved.iter().map(|(path, _)| path.clone()).collect();
        // Read a few at a time, as each is a handful of git commands: on a busy machine,
        // one after another took minutes (9 Nov 2026).
        let mut queue = saved.into_iter();
        let mut reads = tokio::task::JoinSet::new();
        loop {
            while reads.len() < RESTORE_AT_ONCE
                && let Some((path, sessions)) = queue.next()
            {
                let inputs = self.inputs.clone();
                reads.spawn(async move {
                    let read = match git::root(&path).await {
                        Ok(root) => read_repo(root, inputs).await,
                        Err(err) => Err(err),
                    };
                    (path, sessions, read)
                });
            }
            tokio::select! {
                done = reads.join_next() => {
                    let Some(done) = done else { break };
                    let Ok((path, sessions, read)) = done else { continue };
                    self.restoring.remove(&path);
                    let read = match read {
                        Ok(read) => read,
                        Err(err) => {
                            tracing::warn!("not watching {} anymore: {err:#}", path.display());
                            continue;
                        }
                    };
                    let root = read.root.clone();
                    self.watch_read(read, Sessions::default());
                    if let Some(watched) = self.repos.get_mut(&root) {
                        for it in sessions {
                            watched.add_session(it.session);
                        }
                    }
                }
                Some(input) = received.recv() => self.meanwhile(input, &mut held).await,
            }
        }
        while let Ok(input) = received.try_recv() {
            self.meanwhile(input, &mut held).await;
        }
        self.restoring.clear();
        // What zirc's tool hook told an older zircd, which nothing reads any more.
        for gone in [paths::owners(&self.org.state), paths::ran(&self.org.state)] {
            let _ = std::fs::remove_file(gone);
        }
        // Saving over what couldn't be read would lose it.
        if readable {
            self.save();
        }
        self.restore_leases();
        self.restore_requests();
        self.restore_wakes();
        self.restore_awaits();
        self.restore_gotchas();
        held
    }

    /// What's `received` while restoring: the connection coming or going, which daemon
    /// holds a repository, and status requests, now; the rest `held`, as they might save or
    /// announce before it's done.
    async fn meanwhile(&mut self, input: Input, held: &mut Vec<Input>) {
        match input {
            Input::Connected(_)
            | Input::Disconnected
            | Input::Holds(..)
            | Input::Request(Request::Status, _) => {
                self.handle(input).await;
            }
            input => held.push(input),
        }
    }

    /// Whether it watches the repository with this `root`, or will once restored.
    fn holds(&self, root: &Path) -> bool {
        self.repos.contains_key(root) || self.restoring.contains(root)
    }

    pub async fn handle(&mut self, input: Input) {
        match input {
            // A guest's terminal reads its room's history and nothing else's, and trims none.
            Input::Request(request, answer) if self.guest_refuses(&request) => {
                let _ = answer.send(self.guest_refusal());
            }
            Input::Request(Request::Trim { target, id, reason }, answer) => {
                self.trim(target, id, reason.as_deref(), answer);
            }
            Input::Request(read @ Request::Read { .. }, answer) => {
                if let Some(answer) = self.read_kept(&read, answer) {
                    self.read(read, answer);
                }
            }
            Input::Request(Request::Conversations { seconds }, answer) => {
                self.conversations(seconds, answer);
            }
            Input::Request(post @ (Request::Post { .. } | Request::SealedPost { .. }), answer) => {
                self.post(post, answer).await;
            }
            Input::Request(deploy @ Request::Workflow { .. }, answer) => {
                self.workflow(deploy, answer).await;
            }
            Input::Request(request, answer) => {
                let response = self
                    .respond(request)
                    .await
                    .unwrap_or_else(|err| Response::Error {
                        message: format!("{err:#}"),
                    });
                let _ = answer.send(response);
            }
            Input::Watch(heard, session) => {
                tracing::debug!(?session, "a client is watching");
                // A watch hears nothing while the machine can't connect: it's told why.
                if let Some(problem) = &self.problem {
                    let _ = heard.try_send(Response::Error {
                        message: problem.clone(),
                    });
                    return;
                }
                if let Some(session) = &session {
                    self.book.seen(session, now());
                    self.catch_up(&heard, session);
                }
                self.watchers.push(Watcher { heard, session });
            }
            Input::Revoked => self.stopped(zirc_core::ipc::REVOKED.to_owned()),
            Input::Refused(message) => self.stopped(message),
            Input::Connected(client) => {
                self.problem = None;
                self.client = Some(client);
                self.joined.clear();
                self.drop_ops();
                self.replay_connected();
                self.reconcile();
            }
            Input::Disconnected => {
                self.client = None;
                self.replay_disconnected();
                self.batches.clear();
                self.joined.clear();
                self.drop_ops();
                self.presence.clear();
                for asking in self.asking.drain(..) {
                    let _ = asking.answer.send(Response::Error {
                        message: "the connection to the server dropped before it answered"
                            .to_owned(),
                    });
                }
            }
            Input::Expired(token) => self.expired(token),
            Input::Updated(version) => {
                tracing::info!("updated to {PROGRAM} {version}: restarting");
                self.restart();
            }
            Input::Line(line) => {
                // History that neither the replay nor a read takes, as a read's that waited
                // past its time, is nobody's: never told as if just said.
                let history = self.batches.history(&line);
                if !self.replayed(&line) && !self.answers(&line) && !history {
                    self.receive(&line);
                }
            }
            Input::Scanned(root, read) => {
                self.scanning.remove(&root);
                self.scan_done(&root, read);
            }
            Input::PushChecked(clashes) => self.push_checked(clashes),
            Input::Predicted(channel, overlaps) => self.predicted(&channel, overlaps),
            Input::RemotePredicted(predicted) => self.remote_predicted(predicted),
            Input::Holds(root, answer) => {
                let _ = answer.send(self.holds(&root));
            }
            Input::Touched(root) => {
                tracing::trace!("touched {}", root.display());
                if let Some(watched) = self.repos.get_mut(&root) {
                    let now = self.clock.now();
                    let first = watched.touched.map_or(now, |(first, _)| first);
                    watched.touched = Some((first, now));
                }
            }
        }
    }

    /// It's stopped connecting, for the reason `message` says: watches hear it, and end.
    fn stopped(&mut self, message: String) {
        self.problem = Some(message.clone());
        self.tell_watchers(&Response::Error { message });
        self.watchers.clear();
    }

    /// What a guest's terminal answers a request it refuses ([`Daemon::guest_refuses`]).
    fn guest_refusal(&self) -> Response {
        let room = self
            .org
            .guest
            .as_ref()
            .map_or("", |guest| guest.room.as_str());
        Response::Error {
            message: format!(
                "{} is a guest's terminal: it reads {room}'s history alone",
                self.org.name
            ),
        }
    }

    /// Whether a guest's terminal refuses `request`, which waits on the server: anything but
    /// reading its own room.
    fn guest_refuses(&self, request: &Request) -> bool {
        let Some(guest) = &self.org.guest else {
            return false;
        };
        match request {
            Request::Read { target, .. } => !guest.room.eq_ignore_ascii_case(target),
            Request::Trim { .. } | Request::Conversations { .. } => true,
            _ => false,
        }
    }

    /// Stops, to start again from its binary: watches hear so, and watch again once it's
    /// back.
    fn restart(&mut self) {
        self.tell_watchers(&Response::Error {
            message: zirc_core::ipc::RESTARTING.to_owned(),
        });
        self.watchers.clear();
        self.stopping = true;
        self.restarting = true;
    }

    /// The sessions with a watch still open, sorted.
    fn watching(&self) -> Vec<Option<String>> {
        let mut watching: Vec<Option<String>> = self
            .watchers
            .iter()
            .filter(|watcher| !watcher.heard.is_closed())
            .map(|watcher| watcher.session.clone())
            .collect();
        watching.sort();
        watching
    }
}

/// What a request is told when it needs the server and zircd isn't connected to it.
const NOT_CONNECTED: &str = "not connected to the server";

impl Daemon {
    /// The connection to the server, or that there's none ([`NOT_CONNECTED`]).
    fn connected(&self) -> Result<&Client> {
        self.client.as_ref().context(NOT_CONNECTED)
    }
}

/// Keeps `item` among the latest [`QUERIES`].
fn remember<T>(list: &mut VecDeque<T>, item: T) {
    list.push_back(item);
    while list.len() > QUERIES {
        list.pop_front();
    }
}

/// A query's id: short, and different from this machine's others and, very likely, from
/// other machines' too.
fn query_id(count: u64) -> String {
    use std::hash::{BuildHasher as _, Hasher as _};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(count);
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos()),
    );
    format!("q{:07x}", hasher.finish() & 0xfff_ffff)
}

/// How long an overlap told stays told: one that comes again unchanged within this, as
/// when a path stops overlapping for a while and starts again, isn't news.
const TOLD_FOR: Duration = Duration::from_hours(12);

/// How an overlapping path was told: what it said about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Said {
    /// That both change the same lines.
    SameLines,
    /// That they change different parts.
    Apart,
    /// Nothing more than that both change it.
    Both,
    /// That it's a lockfile, which the second to land regenerates.
    Lockfile,
}

/// Each path told of, with what was said of it and when.
type Paths = BTreeMap<String, (Said, Instant)>;

/// The overlaps told lately: for each pair of working sets, or other machine, in a channel,
/// the paths told of.
#[derive(Default)]
struct Told {
    local: BTreeMap<(String, [Worktree; 2]), Paths>,
    remote: BTreeMap<(String, String), Paths>,
}

impl Told {
    /// Leaves `overlap` with only its paths that are news at `now`, remembering them. When
    /// any are, whether they make a clash of what was told as changes in different parts.
    fn local_news(&mut self, overlap: &mut LocalOverlap, now: Instant) -> Option<bool> {
        let key = (overlap.channel.clone(), overlap.worktrees.clone());
        let told = self.local.entry(key).or_default();
        let said = |path: &str| {
            if zirc_core::hunks::is_lockfile(path) {
                Said::Lockfile
            } else if overlap.same_lines.contains_key(path) {
                Said::SameLines
            } else if overlap.apart.iter().any(|it| it == path) {
                Said::Apart
            } else {
                Said::Both
            }
        };
        let mut escalated = false;
        let news: Vec<String> = overlap
            .paths
            .iter()
            .filter(|path| {
                let said = said(path);
                let was_apart = told.get(path.as_str()).is_some_and(|(before, at)| {
                    *before == Said::Apart && now.duration_since(*at) < TOLD_FOR
                });
                let is_news = news(told, path, said, now);
                escalated |= is_news && was_apart && said == Said::SameLines;
                is_news
            })
            .cloned()
            .collect();
        overlap.same_lines.retain(|path, _| news.contains(path));
        overlap.apart.retain(|path| news.contains(path));
        overlap.paths = news;
        (!overlap.paths.is_empty()).then_some(escalated)
    }

    /// The same for an overlap with another machine.
    fn remote(&mut self, overlap: &mut Overlap, now: Instant) -> bool {
        let key = (overlap.channel.clone(), overlap.nick.clone());
        let told = self.remote.entry(key).or_default();
        overlap.paths.retain(|path| {
            let said = if zirc_core::hunks::is_lockfile(path) {
                Said::Lockfile
            } else {
                Said::Both
            };
            news(told, path, said, now)
        });
        !overlap.paths.is_empty()
    }
}

/// Whether saying `said` of `path` at `now` is news, given what was `told`, remembering it if
/// it is: it is when it wasn't told lately, or something else was said of it.
fn news(
    told: &mut BTreeMap<String, (Said, Instant)>,
    path: &str,
    said: Said,
    now: Instant,
) -> bool {
    told.retain(|_, (_, at)| now.duration_since(*at) < TOLD_FOR);
    if told.get(path).is_some_and(|(before, _)| *before == said) {
        return false;
    }
    told.insert(path.to_owned(), (said, now));
    true
}

/// How many message ids zircd remembers having heard.
const HEARD: usize = 4096;

/// The ids of the messages heard lately, so one the server sends again isn't news twice.
#[derive(Default)]
struct Heard {
    ids: BTreeSet<String>,
    order: VecDeque<String>,
}

impl Heard {
    /// Whether `id` is heard for the first time, remembering it.
    fn first_time(&mut self, id: &str) -> bool {
        if !self.ids.insert(id.to_owned()) {
            return false;
        }
        self.order.push_back(id.to_owned());
        if self.order.len() > HEARD
            && let Some(oldest) = self.order.pop_front()
        {
            self.ids.remove(&oldest);
        }
        true
    }
}

/// Whether `name` can be one person's nick, and so names nobody else: not a channel, not a
/// list of targets, nothing IRC reads as more than one word.
fn is_nick(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['#', '&', ':', '$', '+', '!'])
        && !name
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || [',', '*', '?', '@'].contains(&c))
}

/// How often a machine tries again to join the channels it should be in and isn't.
const REJOIN_EVERY: Duration = Duration::from_secs(10);

/// How often a session typing says it still is: as people's apps do, well within the six
/// seconds after which apps take it that it's stopped.
const TYPING_EVERY: Duration = Duration::from_secs(3);

/// A session typing to someone, until when, and when it last said so.
struct Typing {
    until: Instant,
    sent: Instant,
}

/// Which message to react to: by its id (and where it was said, if zircd may not have heard
/// it), or the latest from someone.
struct Which<'a> {
    message: Option<&'a str>,
    last: Option<&'a str>,
    to: Option<&'a str>,
}

/// How many messages heard lately zircd remembers, to react to.
const RECENT: usize = 2000;

/// The shortest start of an id `zirc react` takes for the whole.
const SHORTEST_ID: usize = 6;

/// A message heard lately: its id, where it was said (a channel, or the nick of a direct
/// conversation), and who said it.
#[derive(Debug, Clone)]
struct Seen {
    id: String,
    target: String,
    from: String,
    session: Option<String>,
    /// What it is, for a reaction to it, when it was a message heard in a channel.
    about: Option<zirc_core::ipc::Reacted>,
    /// Its sealed parts, the `+zirc.dev/sealed` tag's value, for `zirc unseal`, when it has
    /// some.
    sealed: Option<String>,
}

/// The messages heard lately, newest last.
#[derive(Debug, Default)]
struct Recent(VecDeque<Seen>);

impl Recent {
    fn heard(
        &mut self,
        id: &str,
        target: &str,
        from: &str,
        session: Option<&str>,
        about: Option<zirc_core::ipc::Reacted>,
    ) {
        self.0.push_back(Seen {
            id: id.to_owned(),
            target: target.to_owned(),
            from: from.to_owned(),
            session: session.map(str::to_owned),
            about,
            sealed: None,
        });
        while self.0.len() > RECENT {
            self.0.pop_front();
        }
    }

    /// Keeps the sealed parts `value` of the message `id` heard last.
    fn sealed(&mut self, id: &str, value: &str) {
        if let Some(said) = self.0.iter_mut().rev().find(|said| said.id == id) {
            said.sealed = Some(value.to_owned());
        }
    }

    /// The message `id` names, whole or by a unique start of at least [`SHORTEST_ID`].
    fn find(&self, id: &str) -> Result<Seen> {
        if let Some(said) = self.0.iter().rev().find(|said| said.id == id) {
            return Ok(said.clone());
        }
        anyhow::ensure!(
            id.len() >= SHORTEST_ID,
            "{id} is too short to name a message: give at least {SHORTEST_ID} of its id"
        );
        let mut matching = self.0.iter().rev().filter(|said| said.id.starts_with(id));
        match (matching.next(), matching.next()) {
            (Some(said), None) => Ok(said.clone()),
            (Some(_), Some(_)) => {
                anyhow::bail!("{id} starts more than one message's id: give more")
            }
            (None, _) => anyhow::bail!(
                "no message heard lately has id {id}: zircd knows the last {RECENT} it heard"
            ),
        }
    }

    /// The latest message from `who`, a nick or a session.
    fn latest_from(&self, who: &str) -> Option<Seen> {
        self.0
            .iter()
            .rev()
            .find(|said| {
                said.from.eq_ignore_ascii_case(who)
                    || said
                        .session
                        .as_deref()
                        .is_some_and(|it| it.eq_ignore_ascii_case(who))
            })
            .cloned()
    }
}

/// How many repositories are read at once.
const SCANS: usize = 4;

/// How much of what concerned a session while it wasn't watching it's told when it watches
/// again: the latest, from no longer ago than [`MISSED_FOR`].
const MISSED: usize = 30;
const MISSED_FOR: Duration = Duration::from_hours(2);

/// The channel `response` was heard in, if it was in one.
fn channel_of(response: &Response) -> Option<&str> {
    match response {
        Response::Delivery(delivery) => Some(&delivery.channel),
        Response::Overlap(overlap) => Some(&overlap.channel),
        Response::LocalOverlap(overlap) => Some(&overlap.channel),
        Response::Conflict(conflict) => Some(&conflict.channel),
        _ => None,
    }
}

/// A socket client watching, and the session it watches as.
struct Watcher {
    heard: mpsc::Sender<Response>,
    session: Option<String>,
}

impl Watcher {
    /// Whether a line sent now would reach the client: not if it hung up, nor if it has
    /// fallen [`crate::socket::WATCH_BUFFER`] lines behind.
    fn reachable(&self) -> bool {
        !self.heard.is_closed() && self.heard.capacity() > 0
    }
}

/// Whether a push of `branch` to `head`, changing `paths`, lands under changes in one of
/// `worktrees`: one that will rebase onto the branch and has changed those paths. One at the
/// pushed commit already has nothing to rebase: the push was its own, as when it landed its
/// branch.
fn lands_under(worktrees: &[&Watched], branch: &str, head: &str, paths: &[String]) -> bool {
    worktrees.iter().any(|watched| {
        let onto =
            branch == watched.default_branch || watched.snapshot.branch.as_deref() == Some(branch);
        onto && watched.snapshot.head.as_deref() != Some(head)
            && paths
                .iter()
                .any(|path| watched.snapshot.dirty.contains(path))
    })
}

/// Fills in which of `overlap`'s paths its two worktrees change on the same lines, and
/// which in different parts, as rebasing one's own work onto the other's would say. A path
/// one of them doesn't change of its own, as when it only has what the default branch did
/// that the other hasn't, or its files were left behind as its branch moved on, goes.
async fn predict(overlap: &mut LocalOverlap) {
    let [a, b] = [&overlap.worktrees[0].path, &overlap.worktrees[1].path];
    let (Some((a_base, a_tree)), Some((b_base, b_tree))) = (
        own_side(a, &overlap.paths).await,
        own_side(b, &overlap.paths).await,
    ) else {
        return;
    };
    let ours = Side {
        base: &a_base,
        tree: &a_tree,
    };
    let theirs = Side {
        base: &b_base,
        tree: &b_tree,
    };
    let Some(rebased) = git::rebase(a, ours, theirs, &overlap.paths).await else {
        return;
    };
    overlap.paths.retain(|path| rebased.both.contains(path));
    overlap.same_lines = rebased.same_lines;
    overlap.apart = rebased.apart;
}

/// The worktree at `root`'s own work in `paths`: the commit of the default branch it's made
/// from, from where its files came from, and a tree of that commit's with its files there.
async fn own_side(root: &Path, paths: &[String]) -> Option<(String, String)> {
    let base = git::files(root).await.ok()?.base?;
    let fork = git::fork(root, &base).await.unwrap_or(base);
    let tree = git::own_tree(root, &fork, paths).await?;
    Some((fork, tree))
}

/// The name of the worktree's directory at `root`.
fn worktree_name(root: &Path) -> Option<String> {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

/// A `PRIVMSG` in `channel` as watchers hear it, and whether it's a zirc message: a zirc
/// message, or plain text a person typed, which concerns a session as a post would. Other
/// CTCPs, such as /me, aren't for watchers, nor are zirc messages that can't be read.
fn heard(channel: &str, line: &Line) -> Option<(Delivery, bool)> {
    let from = line
        .source
        .as_deref()
        .and_then(|source| source.split('!').next())
        .unwrap_or_default();
    let body = line.params.get(1)?;
    let (message, zirc) = match zirc_proto::decode(body) {
        Ok(Some(message)) => (message, true),
        Ok(None) if !body.starts_with('\x01') => (
            Message::new(
                "",
                Event::Text {
                    body: body.to_owned(),
                },
            ),
            false,
        ),
        Ok(None) => return None,
        Err(err) => {
            tracing::debug!("unreadable Zirc message from {from}: {err}");
            return None;
        }
    };
    let tag = |key: &str| line.tag(key).map(str::to_owned);
    let mut delivery = Delivery {
        id: tag("msgid"),
        time: tag("time"),
        reply: tag("+draft/reply"),
        sealed: tag(zirc_proto::sealed::TAG),
        account: tag("account").filter(|account| !account.is_empty() && account != "*"),
        hosted_by: zirc_core::agents::host_owner(line.source.as_deref()).map(str::to_owned),
        client_of: zirc_core::agents::client_owner(line.source.as_deref()).map(str::to_owned),
        ..Delivery::new(channel, from, message)
    };
    only_cis_word(&mut delivery);
    Some((delivery, zirc))
}

/// Makes an event only CI says ([`zirc_proto::ci::only_cis`]) a plain post of its text unless
/// the account the server says sent it is one CI speaks from
/// ([`zirc_proto::ci::speaks_for_ci`]). Done once, as a message is heard or said, so nothing
/// after it (the waits a landing ends, a task's `done:`, the hint under a line) takes a
/// member's or a machine's event for CI's by its kind and wording.
fn only_cis_word(delivery: &mut Delivery) {
    if zirc_proto::ci::speaks_for_ci(delivery.account.as_deref()) {
        return;
    }
    if let Event::Custom { kind, text, .. } = &mut delivery.message.event
        && zirc_proto::ci::only_cis(kind)
    {
        delivery.message.event = Event::Text {
            body: std::mem::take(text),
        };
    }
}

/// Says how sure a decision `delivery` carries is its decider's word, from the account the
/// server says sent it, on a server whose accounts only zirc.dev makes when `zirc_dev`.
fn vouch(delivery: &mut Delivery, zirc_dev: bool) {
    if let Some(decision) = Decision::of(&delivery.message.event) {
        let sender = zirc_core::agents::Sender::of_delivery(delivery);
        let how = decisions::provenance(&decision, sender, zirc_dev);
        delivery.decided = Some(Box::new(how));
    }
}

/// Why an agent can't write to a guest. The gateway drops such lines too; this says why.
const GUESTS: &str = "Agents don't talk to guests; ask a person on staff.";

/// Whether `nick` is a guest's, to write to: named as one, whatever its account, since no
/// member's nick or account can be.
fn is_guest(nick: &str) -> bool {
    zirc_proto::named_as_guest(nick)
}

/// Whether `response` is a line a guest said: someone a public channel let in without an
/// account ([`zirc_proto::is_guest`]).
fn from_guest(response: &Response) -> bool {
    match response {
        Response::Delivery(delivery) => {
            zirc_proto::is_guest(&delivery.from, Some(delivery.account.as_deref()))
        }
        Response::Direct(direct) => zirc_proto::named_as_guest(&direct.from),
        Response::Reaction(reaction) => {
            zirc_proto::is_guest(&reaction.from, Some(reaction.account.as_deref()))
        }
        _ => false,
    }
}

/// The reaction a `TAGMSG` in `target` from `from` gives or takes back (`+draft/react`,
/// `+draft/unreact`), if it's one on a message (`+draft/reply`). Its account is the
/// server's `account-tag`, and nothing else: a nick or another tag isn't anyone's word.
fn reaction(target: &str, from: &str, line: &Line) -> Option<Reaction> {
    let (emoji, removed) = match (line.tag("+draft/react"), line.tag("+draft/unreact")) {
        (Some(emoji), _) => (emoji, false),
        (None, Some(emoji)) => (emoji, true),
        (None, None) => return None,
    };
    let to = line.tag("+draft/reply").filter(|id| !id.is_empty())?;
    if emoji.is_empty() {
        return None;
    }
    Some(Reaction {
        channel: target.to_owned(),
        from: from.to_owned(),
        account: line
            .tag("account")
            .filter(|account| !account.is_empty() && *account != "*")
            .map(str::to_owned),
        emoji: emoji.to_owned(),
        to: to.to_owned(),
        removed,
        time: line.tag("time").map(str::to_owned),
        org: None,
        why: None,
        id: line.tag("msgid").map(str::to_owned),
        on: None,
    })
}

/// Whether `direct` concerns `session`: it names none, or this one, or the watch is the
/// person's own. A query for every session isn't one for the session that asked it.
fn direct_concerns(direct: &Direct, session: Option<&str>) -> Option<Why> {
    match (direct.session.as_deref(), session) {
        (Some(to), Some(watching)) if to != watching => None,
        (None, Some(watching))
            if direct.query.is_some() && direct.from_session.as_deref() == Some(watching) =>
        {
            None
        }
        _ => Some(Why::Direct),
    }
}

/// Whether `answered` concerns `session`: it's the one that asked, or the watch is the
/// person's own.
fn answered_concerns(answered: &Answered, session: Option<&str>) -> Option<Why> {
    session
        .is_none_or(|session| session == answered.request.session)
        .then_some(Why::Decision)
}

/// `response` with `why` it's sent, for what a watcher hears.
fn with_why(response: Response, why: Option<Why>) -> Response {
    match response {
        Response::Delivery(delivery) => Response::Delivery(Delivery { why, ..delivery }),
        Response::Direct(direct) => Response::Direct(Direct { why, ..direct }),
        Response::Overlap(overlap) => Response::Overlap(Overlap { why, ..overlap }),
        Response::LocalOverlap(overlap) => Response::LocalOverlap(LocalOverlap { why, ..overlap }),
        Response::Conflict(conflict) => Response::Conflict(Conflict { why, ..conflict }),
        Response::Reaction(reaction) => Response::Reaction(Reaction { why, ..reaction }),
        Response::Answered(answered) => Response::Answered(Box::new(Answered { why, ..*answered })),
        other => other,
    }
}

/// Why `response` was told, as [`with_why`] set it.
fn why_of(response: &Response) -> Option<Why> {
    match response {
        Response::Delivery(delivery) => delivery.why,
        Response::Direct(direct) => direct.why,
        Response::Overlap(overlap) => overlap.why,
        Response::LocalOverlap(overlap) => overlap.why,
        Response::Conflict(conflict) => conflict.why,
        Response::Reaction(reaction) => reaction.why,
        Response::Answered(answered) => answered.why,
        _ => None,
    }
}

/// Whether `channel` is the repository channel `repo`'s, or one of its branch or topic
/// channels (`#api/login`).
fn in_repo(channel: &str, repo: &str) -> bool {
    channel == repo
        || channel
            .strip_prefix(repo)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether a names reply (`353`: us, the channel's kind, the channel, its names) has us with
/// ops, as `@` before our nick, alone or before others (`@+`).
fn names_us_as_op(params: &[String], me: &str) -> bool {
    params.get(3).is_some_and(|names| {
        names.split(' ').any(|name| {
            name.starts_with('@') && name.trim_start_matches(['@', '+', '%', '&', '~']) == me
        })
    })
}

/// Whether `message`, from `from`, is one of zirc's integrations saying something went as it
/// should: a deploy done or under way, a landing queued or landed, an alert resolved. Failures,
/// conflicts and firing alerts aren't. A line that says its level is routine at info or
/// below; one that doesn't, from CI or a bridge that predates levels, is judged by its words.
fn routine(from: &str, message: &Message) -> bool {
    if zirc_core::chat::sender_kind(from, None) != zirc_core::chat::SenderKind::Integration {
        return false;
    }
    if let Some(level) = message.declared_level() {
        return level <= Level::Info;
    }
    let event = &message.event;
    let text = event.human_text();
    // CI's lines start with the session they're for.
    let said = zirc_proto::said(&text);
    let lower = said.to_lowercase();
    if ["fail", "conflict", "couldn't", "firing", "error"]
        .iter()
        .any(|bad| lower.contains(bad))
    {
        return false;
    }
    [
        "[deployed]",
        "[resolved]",
        "landed ",
        "queued ",
        "deploying ",
    ]
    .iter()
    .any(|good| said.starts_with(good))
        // "build 12's Apple apps pass on 1a2b3c4" after a landing, or "compile on" it, or
        // "compile: covered by" another build: however CI words it, a check of the Apple
        // apps that didn't fail (above) needs nothing.
        || (said.starts_with("build ") && lower.contains("apple apps"))
        || matches!(event, Event::Custom { kind, .. } if kind == zirc_proto::ci::DEPLOY_KIND)
        // Any of CI's events saying it passed ($ZIRC_STATE in ops/say.sh), such as a
        // scheduled run's "build 9's full Apple tests at 1a2b3c4 pass" (6 Nov 2026).
        || matches!(event, Event::Custom { data, .. }
            if data.get("state").and_then(serde_json::Value::as_str) == Some("passed"))
}

/// Whether `channel` is a repository's landings channel, `#<repository>/landings`
/// (ops/lib.sh's `landings`): CI's lines that need no one, each naming the session whose
/// landing it is, which alone it's news to.
fn landings(channel: &str) -> bool {
    channel.ends_with("/landings")
}

/// Whether `text` names any of `names` as a word of its own, ignoring case: `mattias-mbp`
/// in "ask mattias-mbp", but not `cc-1` in `cc-12`.
fn mentions(text: &str, names: &[&str]) -> bool {
    names
        .iter()
        .any(|name| zirc_core::names::mentions(text, name))
}

/// Saves `value` as JSON at `path`, readable by this user only.
fn save_saved<T: serde::Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    paths::write_private(path, &json)
}

/// What's saved in `path`: the default when there's no file yet, `None` when it can't be
/// read, which is logged. A file that isn't what zircd saves is moved aside, to
/// `<name>.unreadable`.
fn read_saved<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Option<T> {
    let json = match std::fs::read(path) {
        Ok(json) => json,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Some(T::default()),
        Err(err) => {
            tracing::warn!("couldn't read {}: {err}", path.display());
            return None;
        }
    };
    match serde_json::from_slice(&json) {
        Ok(saved) => Some(saved),
        Err(err) => {
            // Kept aside, so saving later doesn't lose it.
            let mut name = path.file_name().unwrap_or_default().to_owned();
            name.push(".unreadable");
            let aside = path.with_file_name(name);
            let kept = std::fs::rename(path, &aside).map_or_else(
                |err| format!("couldn't keep it aside: {err}"),
                |()| format!("kept it as {}", aside.display()),
            );
            tracing::warn!("couldn't read {}: {err}; {kept}", path.display());
            None
        }
    }
}

pub(crate) use crate::clock::unix as now;

/// The events to send for a change, in the repository with web address `web`. A HEAD that
/// moved by commits just made is one per commit; one that moved otherwise is none. A working
/// set is announced by the daemon, which knows the whole channel's. `default` is the
/// repository's default branch, which a rebased branch's new commits aren't on.
async fn events(root: &Path, change: Change, web: Option<&str>, default: &str) -> Vec<Event> {
    let repo = web.map(str::to_owned);
    match change {
        Change::Working(_) => Vec::new(),
        Change::HeadMoved { branch, from } => {
            let mut commits = Vec::new();
            for (sha, subject) in git::new_commits(root, from.as_deref()).await {
                let paths = git::changed_paths(root, &format!("{sha}^"), &sha).await;
                commits.push(Event::Commit {
                    branch: branch.clone(),
                    sha,
                    subject,
                    paths,
                    repo: repo.clone(),
                });
            }
            commits
        }
        Change::Pushed { branch, head, old } => {
            let forced = match &old {
                Some(old) => !git::is_ancestor(root, old, &head).await,
                None => false,
            };
            // A rewrite says only what it brought that's new: most are rebases, which used
            // to say every commit and path of the branch again (19% of pushes, 4 Nov).
            let (subjects, paths) = match &old {
                Some(old) if forced => git::rewritten(root, old, &head, default).await,
                _ => (
                    git::pushed_subjects(root, old.as_deref(), &head).await,
                    git::pushed_paths(root, old.as_deref(), &head).await,
                ),
            };
            // The lines it changes, counted in what the branch was before, for machines
            // without its commits: only for a push that adds to the branch.
            let lines = match &old {
                Some(old) if !forced && !paths.is_empty() => {
                    git::hunks(root, old, Some(&head), &paths)
                        .await
                        .into_iter()
                        .map(|(path, hunks)| {
                            let lines = hunks.iter().map(|it| [it.start, it.len]).collect();
                            (path, lines)
                        })
                        .collect()
                }
                _ => BTreeMap::new(),
            };
            vec![Event::Push {
                branch,
                head,
                forced,
                subjects,
                paths,
                base: old.filter(|_| !lines.is_empty()),
                lines,
                repo,
            }]
        }
    }
}

/// What watching a repository needs read first, by [`read_repo`].
struct ReadRepo {
    root: PathBuf,
    /// Its common Git directory ([`git::common_dir`]).
    common: PathBuf,
    project: Project,
    default_branch: String,
    snapshot: Snapshot,
    watcher: RecommendedWatcher,
}

/// Reads the repository at `root` for watching it, telling `inputs` when it changes. It
/// needs nothing of the daemon's, so a restore reads several at once, and its file watcher,
/// whose recursive setup blocks (inotify walks the whole tree), is set up off the runtime.
async fn read_repo(root: PathBuf, inputs: mpsc::UnboundedSender<Input>) -> Result<ReadRepo> {
    let dir_name = root
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let project = Project::new(git::remote_url(&root).await.as_deref(), &dir_name);
    let default_branch = git::default_branch(&root).await;
    let snapshot = git::snapshot(&root).await?;
    let git_dirs = git::git_dirs(&root).await?;
    // The last of them, whether or not it's the worktree's own too.
    let common = git_dirs.last().cloned().context("no Git directory")?;
    let dir = root.clone();
    let watcher = tokio::task::spawn_blocking(move || watch(&dir, git_dirs, inputs))
        .await
        .context("setting up the file watcher")??;
    Ok(ReadRepo {
        root,
        common,
        project,
        default_branch,
        snapshot,
        watcher,
    })
}

/// Watches `root` and its Git directories, and tells the daemon when something in them
/// that matters changes: anything in the working tree, and refs and HEAD in Git's own.
fn watch(
    root: &Path,
    git_dirs: Vec<PathBuf>,
    inputs: mpsc::UnboundedSender<Input>,
) -> Result<RecommendedWatcher> {
    // A linked worktree's common Git directory lives outside its root, so it's watched too.
    let elsewhere: Vec<PathBuf> = git_dirs
        .iter()
        .filter(|dir| !dir.starts_with(root))
        .cloned()
        .collect();
    let touched = root.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        if let Some(path) = event.paths.iter().find(|path| matters(path, &git_dirs)) {
            tracing::trace!("{} changed", path.display());
            let _ = inputs.send(Input::Touched(touched.clone()));
        }
    })?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    for dir in &elsewhere {
        watcher.watch(dir, RecursiveMode::Recursive)?;
    }
    Ok(watcher)
}

/// Whether a change at `path` can change a snapshot. In a Git directory only refs and HEAD
/// do; objects, the index and locks come and go with every command. `git_dirs` is the
/// worktree's own first: in the common one a linked worktree shares, `HEAD` and its log are
/// the main checkout's, and only the refs are this worktree's too.
fn matters(path: &Path, git_dirs: &[PathBuf]) -> bool {
    let Some((at, inside)) = git_dirs
        .iter()
        .enumerate()
        .find_map(|(at, dir)| Some((at, path.strip_prefix(dir).ok()?)))
    else {
        return true;
    };
    let refs = inside == Path::new("packed-refs")
        || inside.starts_with("refs")
        || inside.starts_with("logs/refs");
    let own = at == 0 && (inside == Path::new("HEAD") || inside.starts_with("logs"));
    refs || own
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn says_how_it_is_while_restoring_and_holds_the_rest_till_after() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            paths::repos(home.path()),
            r#"[{"path": "/nowhere/a"}, {"path": "/nowhere/b"}]"#,
        )
        .unwrap();
        let org = Org {
            name: "zirc".into(),
            first: true,
            state: home.path().to_owned(),
            several: false,
            guest: None,
        };
        let (inputs, mut received) = mpsc::unbounded_channel();
        let mut daemon = Daemon::new(
            home.path().to_owned(),
            org,
            "irc.example".into(),
            None,
            inputs.clone(),
        );
        let (status, mut asked) = oneshot::channel();
        inputs
            .send(Input::Request(Request::Status, status))
            .unwrap();
        let (list, mut listed) = oneshot::channel();
        inputs
            .send(Input::Request(Request::RepoList, list))
            .unwrap();
        let held = daemon.restore(&mut received).await;
        assert!(matches!(asked.try_recv(), Ok(Response::Status(_))));
        assert!(listed.try_recv().is_err());
        assert_eq!(held.len(), 1);
    }

    #[tokio::test]
    async fn a_restart_waits_for_its_scans_in_flight_then_gives_up() {
        let home = tempfile::tempdir().unwrap();
        let org = Org {
            name: "zirc".into(),
            first: true,
            state: home.path().to_owned(),
            several: false,
            guest: None,
        };
        let (inputs, mut received) = mpsc::unbounded_channel();
        let mut daemon = Daemon::new(
            home.path().to_owned(),
            org,
            "irc.example".into(),
            None,
            inputs.clone(),
        );
        // A scan that ends in a moment: waited for.
        daemon.scanning.insert("/a".into());
        let later = inputs.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = later.send(Input::Scanned("/a".into(), Err(anyhow::anyhow!("gone"))));
        });
        let start = tokio::time::Instant::now();
        daemon
            .finish_scans(&mut received, Duration::from_secs(5))
            .await;
        assert!(daemon.scanning.is_empty());
        assert!(start.elapsed() < Duration::from_secs(2));
        // One that never ends: given up on.
        daemon.scanning.insert("/b".into());
        let start = tokio::time::Instant::now();
        daemon
            .finish_scans(&mut received, Duration::from_millis(200))
            .await;
        assert!(start.elapsed() >= Duration::from_millis(200));
    }

    /// The whole sets' pace, by zircd's own clock: a channel told at `t` is told again no
    /// sooner than `announce_every` later. The integration test in tests/intents.rs can only
    /// time it by the server's clock, where a busy machine's send queue holds the first line
    /// back (build 1905: 1816ms apart, sent 4s apart), so the pace is checked here.
    #[test]
    fn a_channel_hears_whole_sets_no_sooner_than_announce_every_apart() {
        let home = tempfile::tempdir().unwrap();
        let org = Org {
            name: "zirc".into(),
            first: true,
            state: home.path().to_owned(),
            several: false,
            guest: None,
        };
        let (inputs, _received) = mpsc::unbounded_channel();
        let mut daemon = Daemon::new(
            home.path().to_owned(),
            org,
            "irc.example".into(),
            None,
            inputs,
        );
        daemon.announce_every = Duration::from_secs(4);
        let channel = "#api";
        let told = Instant::now();
        daemon.announce(channel, true, told);

        daemon.announce_soon(channel);
        daemon.announce_due(told + Duration::from_millis(3999));
        assert!(daemon.to_announce.contains(channel), "told again too soon");
        daemon.announce_due(told + Duration::from_secs(4));
        assert!(!daemon.to_announce.contains(channel), "not told when due");
    }

    #[test]
    fn an_integrations_success_is_routine_and_its_failures_arent() {
        let routine = |from: &str, event: &Event| routine(from, &Message::new("ci", event.clone()));
        let text = |body: &str| Event::Text { body: body.into() };
        let custom = |kind: &str, text: &str| Event::Custom {
            kind: kind.into(),
            data: serde_json::Value::Null,
            text: text.into(),
        };
        let alerts = "zirc-alerts";
        for event in [
            text("[deployed] web in staging now runs c89f357: web: storage in MB"),
            text("[resolved] the controller is answering again"),
            custom(
                zirc_proto::ci::DEPLOY_KIND,
                "staging runs c89f357 from storage-mb (cc-1)",
            ),
            custom(
                zirc_proto::ci::LANDING_KIND,
                "cc-1: landed login as 1a2b3c4 (build 12)",
            ),
            custom(zirc_proto::ci::LANDING_KIND, "queued login (build 14)"),
            custom(
                zirc_proto::ci::LANDING_KIND,
                "cc-1: build 12's Apple apps pass on 1a2b3c4",
            ),
            custom(
                zirc_proto::ci::LANDING_KIND,
                "cc-1: build 1467's Apple apps compile on be47ba2",
            ),
            // One build of them for several landings names each session.
            custom(
                zirc_proto::ci::LANDING_KIND,
                "cc-1 (apple/login), cc-2 (apple/rail): build 1467's Apple apps compile on be47ba2",
            ),
            custom(
                zirc_proto::ci::LANDING_KIND,
                "cc-1: build 1502's Apple apps compile: covered by 7b7125c, which holds 2b5286c",
            ),
        ] {
            assert!(routine(alerts, &event), "{event:?}");
        }
        let check = |state: &str, text: &str| Event::Custom {
            kind: "x-zirc/check".into(),
            data: serde_json::json!({ "state": state }),
            text: text.into(),
        };
        assert!(routine(
            alerts,
            &check("passed", "build 9's full Apple tests at 1a2b3c4 pass")
        ));
        assert!(!routine(
            alerts,
            &check(
                "warning",
                "build 9's full Apple tests at 1a2b3c4 didn't finish"
            )
        ));
        for event in [
            text("[firing] the controller isn't answering"),
            custom(
                zirc_proto::ci::DEPLOY_KIND,
                "deploy of web failed: see the log",
            ),
            custom(
                zirc_proto::ci::LANDING_KIND,
                "cc-1: build 13 of login failed in test: //a:b",
            ),
            custom(
                zirc_proto::ci::LANDING_KIND,
                "cc-1: login conflicts with main",
            ),
            text("zircd restarted on the mini"),
            custom(
                zirc_proto::ci::LANDING_KIND,
                "cc-1: build 12's Apple apps failed on 1a2b3c4",
            ),
        ] {
            assert!(!routine(alerts, &event), "{event:?}");
        }
        // A person's or an agent's post is never routine, whatever it says.
        assert!(!routine("mattias-mbp", &text("[deployed] web")));
    }

    #[test]
    fn an_integrations_line_that_says_its_level_is_routine_by_it_not_by_its_words() {
        let at = |level: Option<Level>, body: &str| Message {
            level,
            ..Message::new("ci", Event::Text { body: body.into() })
        };
        let alerts = "zirc-alerts";
        // Words that would read as a failure, said at info, go as they should.
        assert!(routine(
            alerts,
            &at(Some(Level::Info), "error budget report: all fine")
        ));
        assert!(routine(
            alerts,
            &at(Some(Level::Debug), "conflicts checked")
        ));
        // And words that would read as routine, said as warn or error, don't.
        assert!(!routine(
            alerts,
            &at(Some(Level::Warn), "landed login, master ahead of e2e")
        ));
        assert!(!routine(
            alerts,
            &at(Some(Level::Error), "[deployed] web, then it crashed")
        ));
        // Without a level, the words still say.
        assert!(!routine(
            alerts,
            &at(None, "[firing] the controller isn't answering")
        ));
        assert!(routine(
            alerts,
            &at(None, "[resolved] the controller is answering")
        ));
        // CI's state is its level: a deploy waiting for approval waits on someone.
        let pending = Message::new(
            "ci",
            Event::Custom {
                kind: zirc_proto::ci::DEPLOY_KIND.into(),
                data: serde_json::json!({ "state": "pending" }),
                text: "approve for production: web".into(),
            },
        );
        assert!(!routine(alerts, &pending));
        // Never a person's, whatever level it says.
        assert!(!routine(
            "mattias-mbp",
            &at(Some(Level::Debug), "landed login")
        ));
    }

    #[test]
    fn knows_when_a_names_reply_has_it_with_ops() {
        let reply = |names: &str| ["mbp", "=", "#api", names].map(String::from);
        assert!(names_us_as_op(&reply("@mbp"), "mbp"));
        assert!(names_us_as_op(&reply("alice @+mbp"), "mbp"));
        assert!(!names_us_as_op(&reply("@alice mbp"), "mbp"));
        assert!(!names_us_as_op(&reply("+mbp"), "mbp"));
        assert!(!names_us_as_op(&reply("@mbp2"), "mbp"));
    }

    #[test]
    fn ignores_git_internals_but_not_refs() {
        let git = vec![PathBuf::from("/r/.git")];
        assert!(matters(Path::new("/r/src/a.rs"), &git));
        assert!(matters(Path::new("/r/.git/HEAD"), &git));
        assert!(matters(Path::new("/r/.git/refs/heads/main"), &git));
        assert!(!matters(Path::new("/r/.git/objects/ab/cdef"), &git));
        assert!(!matters(Path::new("/r/.git/index.lock"), &git));

        // A linked worktree's: its own HEAD and the shared refs, not the main checkout's HEAD.
        let linked = vec![
            PathBuf::from("/r/.git/worktrees/w"),
            PathBuf::from("/r/.git"),
        ];
        assert!(matters(Path::new("/w/src/a.rs"), &linked));
        assert!(matters(Path::new("/r/.git/worktrees/w/HEAD"), &linked));
        assert!(matters(Path::new("/r/.git/worktrees/w/logs/HEAD"), &linked));
        assert!(matters(Path::new("/r/.git/refs/heads/w"), &linked));
        assert!(matters(Path::new("/r/.git/logs/refs/heads/w"), &linked));
        assert!(!matters(Path::new("/r/.git/HEAD"), &linked));
        assert!(!matters(Path::new("/r/.git/logs/HEAD"), &linked));
        assert!(!matters(Path::new("/r/.git/worktrees/v/HEAD"), &linked));
    }

    #[test]
    fn drops_saved_repositories_it_cant_read_and_keeps_the_rest() {
        let home = tempfile::tempdir().unwrap();
        let path = paths::repos(home.path());
        std::fs::write(
            &path,
            r#"["/old", {"path": "/a", "sessions": [{"session": "cc-1"}]},
                {"path": "/w", "unwatched": true}, {"path": "/b", "session": "cc-2"}]"#,
        )
        .unwrap();
        let saved = read_saved_repos(&path).unwrap();
        let read: Vec<(&str, usize, bool)> = saved
            .iter()
            .map(|it| (it.path.to_str().unwrap(), it.sessions.len(), it.unwatched))
            .collect();
        assert_eq!(read, [("/a", 1, false), ("/w", 0, true), ("/b", 0, false)]);
        // Not set aside as unreadable.
        assert!(path.exists());
    }

    #[test]
    fn reads_topics_saved_as_a_list_as_the_machines() {
        let old: SavedTopics = serde_json::from_str(r##"["#login", "#ops"]"##).unwrap();
        let topics = Topics::from(old);
        assert_eq!(topics["#ops"], BTreeSet::from([None]));
        let saved = serde_json::to_string(&SavedTopics::Joiners(Topics::from([(
            "#ops".to_owned(),
            BTreeSet::from([None, Some("cc-1".to_owned())]),
        )])))
        .unwrap();
        let again = Topics::from(serde_json::from_str::<SavedTopics>(&saved).unwrap());
        assert_eq!(again["#ops"].len(), 2);
    }

    #[test]
    fn tells_an_overlap_again_only_when_it_changes() {
        let worktree = |branch: &str| Worktree {
            path: "/src/api".into(),
            branch: Some(branch.into()),
        };
        let overlap = |paths: &[&str], same: &[&str]| LocalOverlap {
            channel: "#api".into(),
            worktrees: [worktree("login"), worktree("logout")],
            paths: paths.iter().map(|&p| p.to_owned()).collect(),
            same_lines: same.iter().map(|&p| (p.to_owned(), vec![[1, 1]])).collect(),
            apart: Vec::new(),
            why: None,
        };
        let mut told = Told::default();
        let start = Instant::now();
        let mut first = overlap(&["a.rs", "Cargo.lock"], &["Cargo.lock"]);
        assert!(told.local_news(&mut first, start).is_some());
        assert_eq!(first.paths, ["a.rs", "Cargo.lock"]);
        // The same again, as after a while apart: not news.
        let mut again = overlap(&["a.rs", "Cargo.lock"], &[]);
        assert!(
            told.local_news(&mut again, start + Duration::from_secs(600))
                .is_none()
        );
        // a.rs now changes the same lines: that's news; the lockfile still isn't.
        let mut worse = overlap(&["a.rs", "Cargo.lock"], &["a.rs", "Cargo.lock"]);
        assert!(
            told.local_news(&mut worse, start + Duration::from_secs(700))
                .is_some()
        );
        assert_eq!(worse.paths, ["a.rs"]);
        assert_eq!(worse.same_lines.keys().collect::<Vec<_>>(), ["a.rs"]);
        // Long after, it's news again.
        let mut later = overlap(&["Cargo.lock"], &[]);
        assert!(
            told.local_news(&mut later, start + TOLD_FOR + Duration::from_secs(1))
                .is_some()
        );
    }

    #[test]
    fn tells_when_changes_in_different_parts_turn_into_a_clash() {
        let worktree = |branch: &str| Worktree {
            path: "/src/api".into(),
            branch: Some(branch.into()),
        };
        let overlap = |same: &[&str], apart: &[&str]| LocalOverlap {
            channel: "#api".into(),
            worktrees: [worktree("login"), worktree("logout")],
            paths: vec!["a.rs".into()],
            same_lines: same.iter().map(|&p| (p.to_owned(), vec![[1, 1]])).collect(),
            apart: apart.iter().map(|&p| p.to_owned()).collect(),
            why: None,
        };
        let mut told = Told::default();
        let start = Instant::now();
        let mut first = overlap(&[], &["a.rs"]);
        assert_eq!(told.local_news(&mut first, start), Some(false));
        assert!(first.quiet());
        let mut worse = overlap(&["a.rs"], &[]);
        assert_eq!(
            told.local_news(&mut worse, start + Duration::from_secs(60)),
            Some(true)
        );
        assert!(!worse.quiet());
    }

    #[test]
    fn finds_a_message_heard_lately_by_its_id_or_who_said_it() {
        let mut recent = Recent::default();
        recent.heard("01jbx7abcdef", "#api", "ada", None, None);
        recent.heard("01jbx7abzzzz", "#api", "alpha", Some("cc-1"), None);
        recent.heard("02kqr8xyzabc", "bob", "bob", None, None);
        assert_eq!(recent.find("02kqr8").unwrap().target, "bob");
        assert!(
            recent
                .find("01jbx7ab")
                .unwrap_err()
                .to_string()
                .contains("more than one")
        );
        assert!(
            recent
                .find("01j")
                .unwrap_err()
                .to_string()
                .contains("too short")
        );
        assert_eq!(recent.find("01jbx7abcdef").unwrap().from, "ada");
        assert_eq!(recent.latest_from("cc-1").unwrap().id, "01jbx7abzzzz");
        assert_eq!(recent.latest_from("ADA").unwrap().id, "01jbx7abcdef");
        assert!(recent.latest_from("cy").is_none());
    }

    #[test]
    fn knows_a_nick_from_a_channel_or_a_list() {
        for nick in ["ada", "mattias-matt", "cc_1[x]"] {
            assert!(is_nick(nick), "{nick}");
        }
        for not in [
            "",
            "#general",
            "&local",
            "bob,#general",
            ":ada",
            "a b",
            "a*",
            "$*",
        ] {
            assert!(!is_nick(not), "{not}");
        }
    }

    #[test]
    fn hears_each_message_once() {
        let mut heard = Heard::default();
        assert!(heard.first_time("a"));
        assert!(!heard.first_time("a"));
        for i in 0..HEARD {
            heard.first_time(&i.to_string());
        }
        assert!(heard.first_time("a"), "forgotten after {HEARD} others");
    }

    #[test]
    fn keeps_every_session_added_the_last_one_last() {
        let mut sessions = Sessions::default();
        sessions.add("one".into());
        sessions.add("two".into());
        assert_eq!(sessions.latest(), Some("two"));
        sessions.add("one".into());
        assert_eq!(sessions.latest(), Some("one"));
        assert_eq!(sessions.names().collect::<Vec<_>>(), ["two", "one"]);
    }
}
