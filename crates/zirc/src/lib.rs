//! zirc: the CLI. Every command is a short-lived client of zircd over its Unix socket and
//! never opens an IRC connection of its own; `zirc daemon start` starts zircd itself.

mod args;
mod catchup;
mod ci;
mod client;
mod daemon;
mod decide;
mod gotchas;
mod guest;
mod hints;
mod lease;
mod lines;
mod log;
mod logout;
mod paint;
mod playbook;
mod post;
mod queries;
mod react;
mod repo;
mod safety;
mod sealed;
mod search;
mod service;
mod sessions;
mod setup;
mod skill;
mod stack;
mod state;
mod status;
pub mod telemetry;
pub use status::BUSY;
mod updates;
mod wait;
mod watch;
#[cfg(test)]
mod which_org;
mod workflows;

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{ArgAction, Parser, Subcommand};
use zirc_core::paths;
use zirc_proto::PROGRAM;

use crate::args::{OrgArg, duration_arg};
use crate::daemon::DaemonCommand;
use crate::hints::{HintsCommand, hints_command};
use crate::post::{TopicArgs, intend, topic, trim_help};
use crate::queries::AgentsCommand;
use crate::repo::{RepoCommand, repo};
use crate::sessions::{avatar_named, avatars_text, name_session, tool_here};
use crate::setup::{Init, Login};
use crate::skill::{REFERENCE, SkillCommand, refresh_skill, skills_dir};
use crate::status::status;
use crate::updates::say_update;

/// The end of `zirc --help`: what to run first.
const GETTING_STARTED: &str = "\
Getting started:
  zirc login       Log in at zirc.dev, and run zircd at login
  zirc repo add    Watch the repository you're in
  zirc watch       Follow what's worth knowing in it
  zirc tui         Chat in the terminal, as yourself

Or, for a server of your own:
  zirc init --server <host>   Say which server to connect to
  zirc daemon install         Run zircd at login

zirc's files are in ~/.zirc, or $ZIRC_HOME.";

/// Under the help of the commands that take them: how a session shows in apps. They're taken
/// anywhere, as they always were, but said only where they do something: on every command's
/// help they were three long paragraphs about something else.
const IDENTITY: &str = "\
How your session shows in apps (also $ZIRC_SESSION_NAME, _ROLE and _TOOL):
      --name <SHORT>  What it's called, such as login-flow, in place of its id
      --role <ROLE>   The role it plays, from `zirc roles`, such as fixer: apps draw its bot
      --tool <TOOL>   The agent tool it runs in, such as claude-code; told by itself in most";

/// What to type, for each command `zirc --help` lists: shown at the end of its own `--help`,
/// where a description says what a command does and an example says how. Each is a whole
/// command line that runs as written, which a test holds them to, so none goes stale.
const EXAMPLES: &[(&str, &[&str])] = &[
    ("login", &["zirc login", "zirc login acme/web"]),
    ("repo", &["zirc repo add", "zirc repo list"]),
    (
        "watch",
        &[
            "zirc watch",
            "zirc watch --once --session cc-1 --name login-flow --role builder",
        ],
    ),
    ("status", &["zirc status", "zirc status --json"]),
    ("tui", &["zirc tui"]),
    (
        "unseal",
        &[
            "zirc unseal 01jbx7ab.1 --to .env.local",
            "zirc unseal 01jbx7ab.2",
        ],
    ),
    (
        "team-key",
        &[
            "zirc team-key new",
            "zirc team-key add",
            "zirc team-key list",
        ],
    ),
    (
        "post",
        &[
            "zirc post --session cc-1 \"taking: the login flow\"",
            "zirc post --session cc-1 --topic auth \"tokens rotate hourly now\"",
        ],
    ),
    (
        "dm",
        &["zirc dm --session cc-1 \"the tests pass: shall I land it?\""],
    ),
    ("note", &["zirc note --session cc-1 cc-2 \"login landed\""]),
    (
        "ask",
        &["zirc ask --session cc-1 --to mbp/cc-2 \"is the cache keyed by branch?\""],
    ),
    (
        "reply",
        &["zirc reply --session cc-2 q1a2b3c \"yes, by branch and target\""],
    ),
    (
        "request",
        &[
            "zirc request --session cc-1 \"Deploy web to production?\" --yes \"deploys it now\" \
             --no \"leaves it on staging\"",
        ],
    ),
    (
        "decide",
        &["zirc decide --session cc-1 --about deploys --quote \"ship on Fridays too\""],
    ),
    ("who", &["zirc who", "zirc who src/login"]),
    (
        "intend",
        &["zirc intend --session cc-1 src/login -m \"rework the form\" --for 45m"],
    ),
    (
        "lease",
        &[
            "zirc lease",
            "zirc lease emulator --session cc-1 --for 20m --wait",
            "zirc lease release emulator --session cc-1",
        ],
    ),
    (
        "state",
        &[
            "zirc state",
            "zirc state set master broken --session cc-1 --why \"e2e fails since 1a2b3c4\"",
            "zirc state clear master --session cc-1",
        ],
    ),
    (
        "wait",
        &[
            "zirc wait landed login",
            "zirc wait deployed 1a2b3c4 --for 45m",
        ],
    ),
    (
        "stack",
        &[
            "zirc stack api-auth --session cc-1",
            "zirc stack --session cc-1",
        ],
    ),
    (
        "gotcha",
        &[
            "zirc gotcha src/login --session cc-1 -m \"the form tests need the fake clock\"",
            "zirc gotcha src/login",
            "zirc gotcha --retire 01jbx7ab --session cc-1",
        ],
    ),
    ("playbook", &["zirc playbook src/login", "zirc playbook"]),
    ("join", &["zirc join auth --session cc-1"]),
    (
        "workflow",
        &[
            "zirc workflow check .zirc/workflows/master.toml",
            "zirc workflow deploy .zirc/workflows/master.toml",
            "zirc workflow run master owner=cc-1a2b3c",
        ],
    ),
    (
        "role",
        &[
            "zirc role join fixers --session cc-1",
            "zirc role leave fixers --session cc-1",
        ],
    ),
    (
        "step",
        &[
            "zirc step claim master-3 fix --session cc-1",
            "zirc step done master-3 fix cause=\"a typo\" --session cc-1",
            "zirc step answer spend-2 approve yes",
        ],
    ),
    ("catchup", &["zirc catchup --since 3h"]),
    ("search", &["zirc search --since 2d --from ada login"]),
    ("decisions", &["zirc decisions --about deploys"]),
    ("sessions", &["zirc sessions"]),
    ("logout", &["zirc logout", "zirc logout acme"]),
    (
        "agents",
        &["zirc agents queries", "zirc agents queries mine"],
    ),
    ("ci", &["zirc ci token", "zirc ci tokens"]),
    ("daemon", &["zirc daemon install", "zirc daemon restart"]),
    ("update", &["zirc update", "zirc update --check"]),
    ("skill", &["zirc skill install"]),
    ("block", &["zirc block mal"]),
    ("report", &["zirc report mal --reason harassment"]),
];

/// `command`'s examples ([`EXAMPLES`]), as the end of its `--help` shows them, before `more`
/// when it has more to say there.
fn examples(command: &str, more: &str) -> String {
    let lines: Vec<String> = EXAMPLES
        .iter()
        .filter(|(name, _)| *name == command)
        .flat_map(|(_, lines)| lines.iter())
        .map(|line| format!("  {line}"))
        .collect();
    let mut text = format!("Examples:\n{}", lines.join("\n"));
    if !more.is_empty() {
        text.push_str("\n\n");
        text.push_str(more);
    }
    text
}

/// The commands `zirc --help` lists, under what each is for, in the order it shows them. The
/// rest still run and have their own `--help`, and `zirc help --all` lists them: the ones
/// scripts and CI call, the ones another now covers, and the ones few people need. A command
/// is in one group or hidden, never both, which a test holds to.
const GROUPS: &[(&str, &[&str])] = &[
    ("Start", &["login", "repo", "watch", "status", "tui"]),
    (
        "Say",
        &["post", "dm", "note", "ask", "reply", "request", "decide"],
    ),
    (
        "Coordinate",
        &[
            "who", "intend", "lease", "state", "wait", "stack", "gotcha", "playbook", "join",
            "workflow", "role", "step",
        ],
    ),
    (
        "Read",
        &["catchup", "search", "decisions", "sessions", "unseal"],
    ),
    (
        "Manage",
        &[
            "logout", "agents", "ci", "daemon", "update", "skill", "block", "report", "team-key",
        ],
    ),
];

/// The heading `zirc help --all` puts over the commands `--help` leaves out.
const MORE: &str = "More";

/// Every command as clap knows it, without the `Cli` around them: what [`help_template`]
/// lists while `Cli`'s own command is still being made.
fn commands() -> clap::Command {
    <Command as Subcommand>::augment_subcommands(clap::Command::new(PROGRAM))
}

/// The commands under their headings ([`GROUPS`]), a line each with its summary as its own
/// `--help` gives it; with `all`, those left out too, under [`MORE`].
fn listed(all: bool) -> String {
    use std::fmt::Write as _;
    let commands = commands();
    let about = |name: &str| {
        commands
            .find_subcommand(name)
            .and_then(clap::Command::get_about)
            .map(ToString::to_string)
            .unwrap_or_default()
    };
    let more: Vec<&str> = commands
        .get_subcommands()
        .filter(|command| command.is_hide_set())
        .map(clap::Command::get_name)
        .collect();
    let groups = GROUPS
        .iter()
        .copied()
        .chain(all.then_some((MORE, more.as_slice())));
    let width = commands
        .get_subcommands()
        .map(|command| command.get_name().len())
        .max()
        .unwrap_or_default();
    let mut text = String::new();
    for (heading, names) in groups {
        text.push_str(heading);
        text.push_str(":\n");
        for name in names {
            let _ = writeln!(text, "  {name:<width$}  {}", about(name));
        }
        text.push('\n');
    }
    text
}

/// What `zirc --help` shows: the commands by what they're for, not in one long list.
fn help_template() -> String {
    format!(
        "{{about}}\n\n{{usage-heading}} {{usage}}\n\n{}More commands: {PROGRAM} help --all\n\
         A command's own help: {PROGRAM} help <command>{{after-help}}",
        listed(false)
    )
}

/// `zirc help`: zirc's help, every command with `all`, or the help of the command `path`
/// names, such as `repo add`.
fn help(all: bool, path: &[String], out: &mut impl Write) -> Result<()> {
    use clap::CommandFactory as _;
    let mut command = Cli::command();
    command.build();
    if path.is_empty() {
        if all {
            write!(out, "{}", listed(true))?;
        } else {
            write!(out, "{}", command.render_help())?;
        }
        return Ok(());
    }
    let mut found = &mut command;
    for name in path {
        found = found
            .find_subcommand_mut(name)
            .ok_or_else(|| anyhow::anyhow!("{PROGRAM} has no command {}", path.join(" ")))?;
    }
    write!(out, "{}", found.render_long_help())?;
    Ok(())
}

/// Ambient awareness for teams of people and coding agents, over IRC.
// The doc comments on the argument types are --help text, where backticks would show.
#[derive(Debug, Parser)]
#[command(
    name = "zirc",
    version,
    after_help = GETTING_STARTED,
    help_template = help_template(),
    disable_help_subcommand = true
)]
pub struct Cli {
    /// zirc's directory: ~/.zirc unless ZIRC_HOME says otherwise.
    #[arg(long, env = "ZIRC_HOME", global = true, hide = true)]
    home: Option<PathBuf>,
    /// What your session's called, such as zirc-chat or guests-web: apps show it in place
    /// of the session's id. `zirc watch`, `zirc join` and `zirc post` set it for the
    /// session; without one, a session goes by the branch it named first.
    #[arg(
        long,
        env = "ZIRC_SESSION_NAME",
        global = true,
        value_name = "SHORT",
        hide = true
    )]
    name: Option<String>,
    /// The agent tool your session runs in, such as claude-code or codex, so apps can show
    /// its icon. zirc tells from where it runs, in Claude Code or Codex, unless this says.
    #[arg(
        long,
        env = "ZIRC_SESSION_TOOL",
        global = true,
        value_name = "TOOL",
        hide = true
    )]
    tool: Option<String>,
    /// The role your session plays, from `zirc roles`, such as fixer: apps draw the bot
    /// dressed for it. Without it, the role its name tells, else a plain bot. `--avatar`,
    /// from before roles, still takes one. `zirc watch`, `zirc join` and `zirc post` set it
    /// for the session.
    #[arg(
        long = "role",
        alias = "avatar",
        env = "ZIRC_SESSION_ROLE",
        global = true,
        value_name = "ROLE",
        hide = true
    )]
    avatar: Option<String>,
    /// `-?` asks for help too, as `-h` does.
    #[arg(short = '?', action = ArgAction::Help, global = true, hide = true)]
    question_mark: Option<bool>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Log in at zirc.dev and connect this machine to your team.
    ///
    /// Log in at zirc.dev, and set this machine up to connect to your team.
    ///
    /// Shows a code and opens the browser, where you sign in and confirm it. zirc then
    /// writes the team's server to ~/.zirc/config.toml, installs the zirc skill for Claude
    /// Code, and runs zircd at login, restarting it if it's running.
    ///
    /// Given a public channel where you're a guest, such as acme/web, it connects this
    /// machine there as you instead: you let it in from that channel's chat, and the agents
    /// here talk in your room there.
    #[command(after_long_help = examples("login", ""))]
    Login(Login),
    /// Sign this machine out, of everything or of one organisation.
    ///
    /// Sign this machine out: of everything, or of the one organisation you name.
    ///
    /// Asks first (`--yes` doesn't), tells zirc.dev this machine's key is gone, forgets the
    /// sign-in in ~/.zirc/config.toml and stops zircd. Offline it signs out here all the same
    /// and says the key wasn't revoked. Given an organisation, such as acme, it takes that
    /// one off this machine and leaves the others; given a public channel where you're a
    /// guest, such as acme/web, it ends this machine's terminal there only.
    #[command(after_long_help = examples("logout", ""))]
    Logout(logout::Logout),
    /// List the organisations this machine is signed in to, or take one off it.
    #[command(hide = true)]
    Orgs {
        #[command(subcommand)]
        command: Option<logout::Orgs>,
    },
    /// Say which server zircd connects to, in ~/.zirc/config.toml.
    #[command(hide = true)]
    Init(Init),
    /// Show how zircd stands: its connection, nick and channels.
    ///
    /// Show zircd's connection, nick and channels.
    #[command(after_long_help = examples("status", ""))]
    #[command(alias = "whoami")]
    Status {
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
        /// Also say where this session is watched. Defaults
        /// to $ZIRC_SESSION.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
    },
    /// Watch a repository, or stop watching it.
    ///
    /// Watch repositories for edits, commits and pushes, or stop watching them.
    #[command(subcommand)]
    #[command(after_long_help = examples("repo", ""))]
    Repo(RepoCommand),
    /// Say something to the sessions working with you.
    ///
    /// It goes to your repository's agents channel, #<repo>/agents, unless the team's
    /// .zirc/config.toml names another room; a post addressed to a person or naming one
    /// (not an agent session) goes to the repository's channel, for people. --to repo from
    /// an agent session is refused unless the post is for a person (--force posts it anyway).
    /// Line breaks stay: with - for the text, it's read from stdin, such as a
    /// code block.
    #[command(after_help = IDENTITY)]
    #[command(after_long_help = examples("post", IDENTITY))]
    Post(post::PostArgs),
    /// Message one person: your user, or --to another.
    ///
    /// Say something to one person alone: this machine's owner, or --to another.
    ///
    /// How an agent answers its own user: what they send it, from any of
    /// their apps, is the only direct message that reaches it. With --to, it goes to that
    /// person's direct messages in their apps instead, signed with the session.
    #[command(after_long_help = examples("dm", ""))]
    Dm(post::DmArgs),
    /// Record a decision your user made.
    ///
    /// Record a decision your user made, in their words, for every session to find.
    ///
    /// Posted as an x-zirc/decision to #ops and the repository's channel, which every
    /// session's watch hears. With --session, your session relays it: post every decision
    /// your user makes in your session, quoting them. A person can post their own at a
    /// terminal, without --session. `zirc decisions` looks them up.
    #[command(after_long_help = examples("decide", ""))]
    Decide(decide::DecideArgs),
    /// Look up the decisions people made.
    ///
    /// Look up the decisions people made, and how sure each is theirs.
    ///
    /// Read from #ops, newest last. Each says whose account sent it: their own, a terminal
    /// on their machine, a session relaying them, or unverified, which decides nothing. One a
    /// later decision replaced is left out, unless --all.
    #[command(after_long_help = examples("decisions", ""))]
    Decisions(decide::DecisionsArgs),
    /// Ask your user a yes-or-no question.
    ///
    /// Ask your user a yes-or-no question, which they answer with a reaction.
    ///
    /// Posted to #decisions as a card saying what a yes and a no each do. They answer from
    /// any app: 👍 (or ✅ 🙌) yes, 👎 (or ❌) no, ❓ (or 🤔 👀) tell me more. Only a reaction
    /// their own account gives counts, never anyone else's, nor any session's. Their answer
    /// comes to your watch as `#decisions <them> answered yes (👍) to <id>: …`; with --wait,
    /// this waits for it and prints it. With --irreversible a yes is held a minute before it
    /// reaches you, which a 👎 stops. On a ❓, tell them more with `zirc dm`.
    #[command(after_long_help = examples("request", ""))]
    Request(decide::RequestArgs),
    /// Wait on a landing, a build, a branch or a state, to be woken when it moves.
    ///
    /// For a wait you can see and stop, zirc wait: a command that ends when it happened.
    /// Other sessions' landings are no news to your watch otherwise. Such as zirc wait-for 1299 for build 1299, or zirc wait-for cc-598aa1. The wait ends
    /// when it lands, fails or conflicts, or after three hours. A post or intent saying
    /// you're waiting for or blocked on a build or a session registers it too. With
    /// --state master, the first state said of master (zirc state) at another level than
    /// it stands at now wakes your watch, once: master fine again, while it's broken, with
    /// no loop on zirc state --check. With nothing to wait on, it says what you wait on,
    /// as your watch does each time it starts.
    #[command(hide = true)]
    WaitFor(sessions::WaitForArgs),
    /// Wait until a landing lands, a deploy is out or a state changes.
    ///
    /// Wait until CI says something happened, in a command that ends when it has.
    ///
    /// Run it in the background, as your watch: its ending wakes you, with CI's line, and
    /// stopping it stops the wait. It ends at once when it happened already, and as failed
    /// when CI says it failed or the wait ran out, so zirc wait … && … goes on only when
    /// it happened. Never a loop on a channel's history.
    #[command(after_long_help = examples("wait", ""))]
    Wait {
        #[command(subcommand)]
        command: wait::WaitCommand,
    },
    /// Build on another session's branch before it lands.
    ///
    /// Build on another session's branch before it lands, and be told when to rebase.
    ///
    /// Such as zirc stack api-auth, run in your worktree once your branch holds
    /// origin/api-auth. Your watch wakes when it lands or a push moves it, and zirc stack
    /// then says the rebase to run: a landing rebases a branch, so its commits come back
    /// with new hashes. Without a branch, it says what yours is stacked on; --check fails
    /// while you shouldn't land yet, for a landing script to stop on.
    #[command(after_long_help = examples("stack", ""))]
    Stack(stack::StackArgs),
    /// Keep a lesson about some paths, or list those kept.
    ///
    /// Keep a lesson about some paths for whoever changes them next, or list them.
    ///
    /// Without -m, it lists the lessons kept, newest first: about a path, such as zirc gotcha
    /// crates/zircd, or all of the repository's.
    ///
    /// Such as zirc gotcha crates/zircd -m "clippy's 100-line limit counts the whole
    /// function: split it first". One line: what goes wrong, what to do instead. It goes to
    /// the repository's gotchas room, not the repository; zirc intend says those under the
    /// paths it names, and zircd tells a session those under what it starts changing.
    #[command(after_long_help = examples("gotcha", ""))]
    Gotcha(gotchas::GotchaArgs),
    /// List the lessons sessions kept about a path, newest first (zirc gotcha).
    #[command(hide = true)]
    Gotchas(gotchas::GotchasArgs),
    /// Show the team's playbook for a path: its hints and gotchas.
    ///
    /// Show what agents hear about a path: the team's hints and the gotchas kept there.
    ///
    /// Hints say what to do when something happens, from your ~/.zirc/config.toml and the
    /// repository's .zirc/hints.toml on its default branch, over zirc's own, which it counts
    /// (zirc hints lists them). They apply throughout the repository. Gotchas are lessons
    /// about paths, newest first: those about the path, what's in it and what holds it.
    #[command(after_long_help = examples("playbook", ""))]
    Playbook(playbook::PlaybookArgs),
    /// Leave a note for one agent session.
    ///
    /// A report or a heads-up only it need read.
    /// <TO> is a session's id or the name it goes by, such as cc-272fe8 or coordinator, or
    /// <machine>/<session>; zircd finds the machine it works on. Its `zirc watch` alone
    /// prints it, as `note from <you>: …`, if its machine lets yours query its agents (`zirc
    /// agents queries`). It asks for no answer: for one, use `zirc ask`. A plain `zirc post`
    /// addressed to one agent session (`coordinator: …`) goes as a note too.
    #[command(after_long_help = examples("note", ""))]
    Note(queries::NoteArgs),
    /// Ask another agent a question.
    ///
    /// Ask other agents something: a machine's, one session's, or a person's.
    ///
    /// --to names a machine, one session on it (<machine>/<session>), or a person, for the
    /// agents on each of their machines here. Their `zirc watch` prints it as `query from <you> [<id>]: …`, if their machine lets
    /// yours query its agents (`zirc agents queries`): by default only the same person's
    /// machines and sessions may. Their answers come to your watch as `answer from …`.
    #[command(after_long_help = examples("ask", ""))]
    Ask(queries::AskArgs),
    /// Answer an agent's question, by its id.
    ///
    /// Answer a query your watch printed, by its id.
    #[command(after_long_help = examples("reply", ""))]
    Reply(queries::ReplyArgs),
    /// Choose who may ask this machine's agents.
    ///
    /// Who may query this machine's agents with `zirc ask`.
    #[command(subcommand)]
    #[command(after_long_help = examples("agents", ""))]
    Agents(AgentsCommand),
    /// Send an event of a kind your team made up, such as a deploy or who's on call.
    ///
    /// Its kind is x-<team>/<name>, such as x-acme/deploy, and it carries the JSON given
    /// with --json. Plain IRC clients and Zirc's apps show its text; scripts and agents pick
    /// it out with `zirc watch --filter`. It has to fit on one IRC message, a few hundred
    /// bytes in all. See `zirc reference`.
    #[command(hide = true)]
    Emit(post::EmitArgs),
    /// Say what you're about to change.
    ///
    /// Say what you're about to change, before you start changing it.
    ///
    /// Whoever else is changing it hears now, rather than when you collide. It goes to the repository's channel and lasts until it expires, a new one replaces
    /// it, or you post `done: …` or `dropped: …`.
    #[command(after_long_help = examples("intend", ""))]
    Intend {
        /// Files or directories you're about to change.
        #[arg(required = true, num_args = 1..)]
        paths: Vec<PathBuf>,
        /// What you're about to do, in a line.
        #[arg(short, long)]
        message: String,
        /// How long it lasts, such as 45m or 2h: 30m unless said.
        #[arg(long = "for", value_name = "TIME", default_value = "30m", value_parser = duration_arg)]
        seconds: u64,
        /// Who's about to do it, such as an agent session's id. Defaults to $ZIRC_SESSION,
        /// then $USER.
        #[arg(long, env = "ZIRC_SESSION")]
        session: Option<String>,
    },
    /// Say which other sessions are changing, or about to change, these paths.
    ///
    /// Their uncommitted changes there and their intents, as `zirc who` knows them, and that
    /// the two of you should agree who lands first. Nothing when nobody is. `//ops:land` runs
    /// it with the branch's paths before it queues a landing; `zirc intend` says the same.
    #[command(hide = true)]
    Clashes(post::ClashesArgs),
    /// Say what your session is called: its short name, and its full title.
    ///
    /// Run once as a session starts, as `zirc name --session <s> --name <short> --title "<full
    /// title>"`. The short name (--name, an IRC-safe handle) is what apps show; the title is
    /// on the card they show for it, sent to the channels the session works in and again
    /// whenever it changes. --link, where its owner opens the session (such as a claude://
    /// link), stays on this machine: only the owner's own apps get it, over zircd's socket.
    /// With nothing to set, it prints what the session goes by: its name, or its branch's
    /// when it gave none.
    #[command(hide = true)]
    #[command(after_help = IDENTITY)]
    Name(sessions::NameArgs),
    /// Print the id of the task your session is on, if it's on one.
    ///
    /// Your `zirc post "taking: …"` starts a task with a new id, which your posts, intents,
    /// commits and pushes then carry, and `//ops:land` puts on your landing; your `done:` or
    /// `dropped:` post ends it, though a landing soon after still claims it. A `zirc intend`
    /// while you're on none starts one too, which your landing ends. Prints nothing while
    /// you're on none.
    #[command(hide = true)]
    Task(sessions::TaskArgs),
    /// Show how often this machine's sessions' watches woke, and acted on it.
    ///
    /// Counts by reason, kept on this machine only: how many times a watch woke, and how
    /// many of those the session followed within 15 minutes by posting, committing, pushing
    /// or another zirc command that does something.
    #[command(args_conflicts_with_subcommands = true)]
    #[command(hide = true)]
    Wakes(sessions::WakesArgs),
    /// Show a person or channel that you're thinking, until you say something there.
    ///
    /// For an agent preparing to answer a direct message, a question, a query or its user's
    /// 👀 on a message: their apps show "<session> is thinking…" until your answer arrives or
    /// the time runs out. Never for channel chatter.
    #[command(alias = "typing")]
    #[command(hide = true)]
    Thinking(post::ThinkingArgs),
    /// Join a topic channel.
    ///
    /// Join a topic channel, shared with the sessions that join it too.
    ///
    /// The repository's topics are #<repo>/<topic>; with --org, the organisation's are
    /// #<topic>.
    #[command(after_help = IDENTITY)]
    #[command(after_long_help = examples("join", IDENTITY))]
    Join(TopicArgs),
    /// Leave a topic channel.
    #[command(hide = true)]
    Leave(TopicArgs),
    /// Show who is working on what, or on the paths you name.
    ///
    /// Show who is working on what, or who else is changing the paths you name.
    ///
    /// Each machine's uncommitted changes by branch, what sessions are about to change and
    /// have taken on, and what's landed lately. Given files or directories, such as zirc who
    /// src/login, only who else is changing or about to change them, and who changed them
    /// lately, to ask: run it before you change them. Nothing but a line saying so when
    /// nobody is.
    #[command(after_long_help = examples("who", ""))]
    Who(sessions::WhoArgs),
    /// Read what was said lately.
    ///
    /// What people and agents said lately, in this machine's conversations.
    ///
    /// From the server's history, grouped by conversation: what names you, then direct
    /// messages, then each channel, the newest of each with a count of the rest. zircd's
    /// own events, such as working sets, are left out.
    #[command(after_long_help = examples("catchup", ""))]
    #[command(alias = "read")]
    Catchup(catchup::CatchupArgs),
    /// React to a message with an emoji.
    ///
    /// React to a message with an emoji, as people do in Zirc's apps.
    ///
    /// Name the message by the id zirc watch shows ([id 01jbx7ab]), or with --last the
    /// latest from someone: such as zirc react --last ada :eyes:.
    #[command(hide = true)]
    React(react::ReactArgs),
    /// The hints zirc watch shows under events, saying what to do, and their layers.
    ///
    /// A hint comes only from zirc's defaults, the repository's .zirc/hints.toml and your own
    /// config, never from anything said in chat. With
    /// `check <file>`, it says what's wrong with a hints file, such as a repository's
    /// .zirc/hints.toml.
    #[command(hide = true)]
    Hints {
        #[command(subcommand)]
        check: Option<HintsCommand>,
    },
    /// Search the team's history.
    ///
    /// Search the team's history, as the organisation lets you read it, from zirc.dev.
    ///
    /// Messages people and agents said, in every channel and conversation the organisation's
    /// policy lets you read, oldest first: such as zirc search --since 2d --from ada login.
    #[command(after_long_help = examples("search", ""))]
    Search(search::SearchArgs),
    /// Report a person or a message.
    ///
    /// Report a person or a message to the organisation's owners and to zirc.
    ///
    /// Such as zirc report mal --reason harassment, or with --message <id> --in <channel> for
    /// one message (its id is in `zirc search`'s output). Never sends what was said: the
    /// owners read it on their dashboard. Reports are acted on within 24 hours.
    #[command(after_long_help = examples("report", ""))]
    Report(safety::ReportArgs),
    /// Block a person.
    ///
    /// Block a person: their messages and notifications stop reaching you.
    #[command(after_long_help = examples("block", ""))]
    Block(safety::WhoArgs),
    /// Unblock a person you blocked.
    #[command(hide = true)]
    Unblock(safety::WhoArgs),
    /// Whom you've blocked.
    #[command(hide = true)]
    Blocked(safety::BlockedArgs),
    /// Take turns on something shared, such as an emulator.
    ///
    /// Lease something your machines share, such as an emulator, one session at a time.
    ///
    /// Such as a simulator or a heavy local build too; the others wait, queued in order.
    /// Without a name, it shows who holds and who's waiting for each lease.
    /// It's yours for 20 minutes unless --for says otherwise, and ends by itself; asking again
    /// while you hold it holds it longer. Waiting, your `zirc watch` says when it's yours.
    /// zircd agrees leases with every machine in the organisation, through its channel.
    #[command(after_long_help = examples("lease", ""))]
    Lease(lease::LeaseArgs),
    /// Check, deploy or start a workflow your team runs.
    ///
    /// A workflow is a file in .zirc/workflows/ whose steps the server gives, one by one, to
    /// the sessions and people who do them, as the nick the file names, in chat where
    /// everyone sees each run's card. `zirc reference` has the file's shape.
    #[command(subcommand)]
    #[command(after_long_help = examples("workflow", ""))]
    Workflow(workflows::WorkflowCommand),
    /// Take on, finish or answer a step a workflow gave you.
    ///
    /// When a workflow's nick names you for a step, take it with `claim`, then say it's
    /// `done` with its outputs, or that it `fail`ed. An approval is answered with `answer`,
    /// or with 👍 or 👎 on its message, only from a person's own account.
    #[command(subcommand)]
    #[command(after_long_help = examples("step", ""))]
    Step(workflows::StepCommand),
    /// Join or leave one of the workflows' roles, to be given its steps.
    ///
    /// A workflow step for a role, such as `who = "role:fixers"`, goes to one of the live
    /// sessions that joined it, the least busy first, and to the next when they're quiet.
    /// A session's roles end with its done: or dropped:, or when its watch stops.
    #[command(subcommand)]
    #[command(after_long_help = examples("role", ""))]
    Role(workflows::RoleCommand),
    /// See or say how something shared stands, such as a broken master.
    ///
    /// See or say how something your team shares stands, such as a broken master.
    ///
    /// Without a name, every state of the repository this is run in that isn't ok; with
    /// one, that state however it stands. States are said with `zirc state set` by anyone,
    /// a team's CI scripts included, and kept in #ops, as decisions are. To be woken when one
    /// is fine, zirc wait state master ok in the background, never a loop on this.
    /// --check fails when one shown is broken, for a one-off check in a CI script.
    #[command(after_long_help = examples("state", ""))]
    State(state::StateArgs),
    /// Show who holds and who's waiting for each lease.
    #[command(hide = true)]
    Leases {
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        org: OrgArg,
    },
    /// Keep the keys that seal parts of messages for your organisations.
    ///
    /// A part of a post between || markers, such as `zirc post "the key is ||sk_test_1||"`,
    /// goes sealed with the organisation's key: only devices holding it can read it, never
    /// the server, push or plain IRC clients. One key per organisation, handed on in person
    /// or on a call.
    #[command(after_long_help = examples("team-key", ""))]
    TeamKey {
        #[command(subcommand)]
        command: sealed::TeamKey,
    },
    /// Print a part of a message sealed with your organisation's key.
    ///
    /// Name it as `zirc watch` shows it, 🔒[sealed 01jbx7ab.1]: `zirc unseal 01jbx7ab.1`.
    /// Prints only the value, to redirect into a file or a command, or writes it with --to,
    /// so it stays out of a transcript. Print it to the terminal only when someone asks.
    #[command(after_long_help = examples("unseal", ""))]
    Unseal(sealed::UnsealArgs),
    /// Remove a message from the server's history, such as a secret posted by mistake.
    #[command(long_about = trim_help())]
    #[command(hide = true)]
    Trim(post::TrimArgs),
    /// Follow what concerns you, as it happens.
    ///
    /// Print what's worth knowing in your repositories' channels, as it happens.
    ///
    /// One line each, until stopped, and only what concerns you: someone else changing or
    /// about to change files you've changed, pushes that change them where you'll rebase or
    /// clash with your lines, messages to your machine or session, and posts that name you,
    /// your files or a topic you joined, from Zirc or any IRC client. For an agent, run
    /// in the background with --once, which ends at the first, to wake it; --all prints
    /// everything.
    #[command(after_help = IDENTITY)]
    #[command(after_long_help = examples("watch", IDENTITY))]
    Watch(watch::WatchArgs),
    /// Show what's been said on the team's server lately, from its log on zirc.dev.
    ///
    /// The last 12 hours by default. For an organisation's owners' machines, while the owners
    /// have debug logging on, such as for an agent to summarise what's happening:
    /// zirc log --since 1d --json
    #[command(hide = true)]
    Log(log::LogArgs),
    /// Manage the tokens your CI reports builds with.
    ///
    /// Make, list or revoke the tokens your CI says how builds went with.
    #[command(subcommand)]
    #[command(after_long_help = examples("ci", ""))]
    Ci(ci::CiCommand),
    /// Chat in the terminal, as yourself.
    ///
    /// Chat in the terminal, signed in as yourself.
    ///
    /// A chat client with a connection of its own, as you rather than this machine: the
    /// first time, it signs you in through zirc.dev.
    #[command(after_long_help = examples("tui", ""))]
    Tui(Box<zirc_tui::Cli>),
    /// Show what every session is doing.
    ///
    /// Show what every session is doing: its task, branch, landing and what it waits on.
    ///
    /// From what zircd hears in your repositories' channels: what sessions said they're
    /// taking on and about to change, their working sets, their posts and the landing
    /// queue's events. Most lately heard from first; one quiet for a day with nothing taken
    /// on is left out.
    #[command(after_long_help = examples("sessions", ""))]
    Sessions(sessions::SessionsArgs),
    /// Print instructions for coding agents, for CLAUDE.md or AGENTS.md.
    ///
    /// Such as: zirc agent-guide >> CLAUDE.md
    #[command(hide = true)]
    AgentGuide,
    /// List the roles an agent session can play, for `--role <role>`, each with its bot.
    #[command(hide = true, alias = "avatars")]
    Roles,
    /// Print the reference for building bots and scripts on zirc.
    ///
    /// The team's IRC server, Zirc's messages and events over it, and zircd's socket, with
    /// how each is versioned.
    #[command(hide = true)]
    Reference,
    /// Install or remove the agent guide as a skill.
    ///
    /// Install the agent guide as a Claude Code skill, or remove it.
    #[command(subcommand)]
    #[command(after_long_help = examples("skill", ""))]
    Skill(SkillCommand),
    /// Start, stop or install zircd.
    ///
    /// Start, stop, or install zircd as a login service.
    #[command(subcommand)]
    #[command(after_long_help = examples("daemon", ""))]
    Daemon(DaemonCommand),
    /// Update zirc and zircd.
    ///
    /// Update zirc and zircd to the latest release, and restart zircd.
    ///
    /// Only where zirc's installer put them: other builds, such as cargo's, update the way
    /// they were installed. zircd also does this by itself about once a day; to turn that
    /// off, put `auto_update = false` in ~/.zirc/config.toml.
    #[command(after_long_help = examples("update", ""))]
    Update {
        /// Only say whether there's a newer release.
        #[arg(long)]
        check: bool,
    },
    /// Show help for zirc or one of its commands; --all lists every command.
    #[command(hide = true)]
    Help {
        /// List every command, those --help leaves out too.
        #[arg(long)]
        all: bool,
        /// The command to show help for, such as post, or repo add.
        command: Vec<String>,
    },
}

/// Whether `err` is only that whatever read zirc's output stopped reading it, as `zirc
/// status | head` does once it has its line: not a failure to report.
#[must_use]
pub fn stopped_reading(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
    })
}

/// Runs `cli`, writing what it prints to `out`.
///
/// # Errors
///
/// When the command fails, including when zircd isn't running for a command that needs it.
pub async fn run(cli: Cli, out: &mut (impl Write + Send)) -> Result<()> {
    // zirc's own home, not a test's or a throwaway one, whose runs leave the real skill be.
    let own_home = cli.home.is_none();
    let home = cli.home.unwrap_or_else(paths::home);
    let socket = paths::socket(&home);
    let tool = cli.tool.clone().or_else(tool_here);
    // ZIRC_SESSION_AVATAR, from before roles, still counts.
    let avatar = cli
        .avatar
        .clone()
        .or_else(|| std::env::var("ZIRC_SESSION_AVATAR").ok())
        .as_deref()
        .map(role_named);
    let name = (cli.name.as_deref(), tool.as_deref(), avatar);
    before(&cli.command, &home, own_home);
    match cli.command {
        Command::Login(args) => match args.target.as_deref().map(guest::target).transpose()? {
            Some(target) => guest::login(&home, args, target, out).await,
            None => setup::login(&home, args, out).await,
        },
        Command::Logout(args) => logout::logout(&home, args, out).await,
        Command::Orgs { command } => logout::orgs(&home, command, out).await,
        Command::Init(init) => setup::init(&home, init, out),
        Command::Status { json, session } => {
            let out = &mut paint::Lines::new(out, paint::status, !json && paint::wanted());
            status(&socket, json, session.as_deref(), out).await
        }
        Command::Repo(command) => repo(&socket, command, out).await,
        Command::Post(args) => post::post(&socket, name, args, out).await,
        Command::Dm(args) => post::dm(&socket, args, out).await,
        Command::Decide(args) => decide::decide(&socket, args, out).await,
        Command::Decisions(args) => decide::decisions(&socket, args, out).await,
        Command::Request(args) => decide::request(&socket, args, out).await,
        Command::WaitFor(args) => sessions::wait_for(&socket, args, out).await,
        Command::Wait { command } => wait::wait(&socket, command, out).await,
        Command::Stack(args) => stack::stack(&socket, args, out).await,
        Command::Workflow(command) => workflows::workflow(&socket, command, out).await,
        Command::Step(command) => workflows::step(&socket, command, out).await,
        Command::Role(command) => workflows::role(&socket, command, out).await,
        Command::Gotcha(args) => gotchas::gotcha(&socket, args, out).await,
        Command::Gotchas(args) => gotchas::list(&socket, args, out).await,
        Command::Playbook(args) => playbook::playbook(&home, &socket, args, out).await,
        Command::Note(args) => queries::note(&socket, args, out).await,
        Command::Ask(args) => queries::query(&socket, args, out).await,
        Command::Reply(args) => queries::reply(&socket, args, out).await,
        Command::Agents(command) => queries::agents(&socket, command, out).await,
        Command::Emit(args) => post::emit(&socket, args, out).await,
        Command::Join(args) => {
            name_session(&socket, args.session.as_deref(), name).await?;
            topic(&socket, args, true, out).await
        }
        Command::Leave(args) => topic(&socket, args, false, out).await,
        Command::Intend {
            paths,
            message,
            seconds,
            session,
        } => intend(&socket, paths, message, seconds, session, out).await,
        Command::Clashes(args) => post::clashes(&socket, args, out).await,
        Command::Lease(args) => {
            let out = &mut paint::Lines::new(out, paint::lease, args.lists() && paint::wanted());
            lease::lease(&socket, args, out).await
        }
        Command::State(args) => state::state(&socket, args, out).await,
        Command::Leases { json, org } => lease::leases(&socket, json, org, out).await,
        Command::Name(args) => sessions::name(&socket, name, args, out).await,
        Command::Task(args) => sessions::task(&socket, args, out).await,
        Command::Wakes(args) => sessions::wakes(&socket, args, out).await,
        Command::Who(args) => {
            let out = &mut paint::Lines::new(out, paint::who, args.lists() && paint::wanted());
            sessions::who(&socket, args, out).await
        }
        Command::Search(args) => search::search(&home, &socket, args, out).await,
        Command::Report(args) => safety::report(&home, &socket, &args, out).await,
        Command::Block(args) => safety::block(&home, &socket, &args, true, out).await,
        Command::Unblock(args) => safety::block(&home, &socket, &args, false, out).await,
        Command::Blocked(args) => safety::blocked(&home, &socket, &args, out).await,
        Command::React(args) => react::react(&socket, args, out).await,
        Command::TeamKey { command } => sealed::team_key(&home, &socket, command, out).await,
        Command::Unseal(args) => sealed::unseal(&socket, args, out).await,
        Command::Hints { check } => hints_command(&home, check, out),
        Command::Catchup(args) => catchup::catchup(&socket, &home, args, out).await,
        Command::Trim(args) => post::trim(&socket, args, out).await,
        Command::Watch(args) => watch::watch(&socket, &home, name, args, out).await,
        // Boxed: the client's future would make every command's as large.
        Command::Tui(tui) => Box::pin(tui.run(&home, out)).await,
        Command::Sessions(args) => {
            let out = &mut paint::Lines::new(out, paint::sessions, !args.json && paint::wanted());
            sessions::sessions(&socket, args, out).await
        }
        Command::Daemon(command) => daemon::daemon(&home, &socket, command, out).await,
        Command::Update { check } => updates::update(&home, check, out).await,
        Command::Thinking(args) => post::thinking(&socket, args, out).await,
        Command::Log(args) => log::log(&home, &socket, args, out).await,
        Command::Ci(command) => ci::ci(&home, &socket, command, out).await,
        Command::AgentGuide => Ok(write!(out, "{}", skill::agent_guide(&home))?),
        Command::Reference => Ok(write!(out, "{REFERENCE}")?),
        Command::Roles => Ok(write!(out, "{}", avatars_text())?),
        Command::Skill(command) => skill::install_or_remove(command, out),
        Command::Help { all, command } => help(all, &command, out),
    }
}

/// What every command does first: says when a newer release is waiting, and after an update
/// brings an installed skill up to this zirc.
fn before(command: &Command, home: &Path, own_home: bool) {
    if !matches!(command, Command::Update { .. }) {
        say_update(home);
    }
    // After an update, the first run of the new zirc brings an installed skill up to it: the
    // old one, which did the update, only knew its own guide.
    if own_home
        && !matches!(command, Command::Skill(_))
        && let Ok(skills) = skills_dir(None)
        && refresh_skill(&skills)
    {
        eprintln!(
            "{PROGRAM}: updated the {PROGRAM} skill to {}",
            env!("CARGO_PKG_VERSION")
        );
    }
}

/// The role `--role` names, saying on stderr which one it took for a word that isn't one.
fn role_named(id: &str) -> &'static str {
    let (role, note) = avatar_named(id);
    if let Some(note) = note {
        eprintln!("{PROGRAM}: {note}");
    }
    role
}

/// The longest line, in characters, a post reads easily with.
const LONG_LINE: usize = 200;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_commands() {
        for args in [
            &["zirc", "status"][..],
            &["zirc", "repo", "add"],
            &["zirc", "repo", "list", "--json"],
            &["zirc", "post", "taking", "the", "login", "flow"],
            &["zirc", "watch", "--json"],
            &["zirc", "watch", "--all"],
            &["zirc", "who"],
            &["zirc", "agent-guide"],
            &["zirc", "post", "--to", "org", "hi"],
            &["zirc", "post", "--topic", "auth", "--org", "hi"],
            &["zirc", "join", "auth"],
            &["zirc", "leave", "auth", "--org"],
            &["zirc", "skill", "install"],
            &["zirc", "ci", "token", "--name", "github"],
            &["zirc", "ci", "tokens"],
            &["zirc", "ci", "revoke", "github"],
            &["zirc", "skill", "uninstall"],
            &["zirc", "daemon", "start"],
            &["zirc", "init", "--server", "irc.zirc.dev"],
        ] {
            Cli::try_parse_from(args).unwrap_or_else(|err| panic!("{args:?}: {err}"));
        }
        assert!(Cli::try_parse_from(["zirc", "post"]).is_err());
        let Command::Note(queries::NoteArgs { to, text, .. }) = Cli::try_parse_from([
            "zirc",
            "note",
            "--session",
            "cc-1",
            "coordinator",
            "landed",
            "it",
        ])
        .unwrap()
        .command
        else {
            panic!("not a note");
        };
        assert_eq!(
            (to.as_str(), text.join(" ").as_str()),
            ("coordinator", "landed it")
        );
        assert!(Cli::try_parse_from(["zirc", "note", "coordinator"]).is_err());
        // A topic is its own target, and --org on a post only qualifies a topic.
        assert!(
            Cli::try_parse_from(["zirc", "post", "--to", "repo", "--topic", "a", "hi"]).is_err()
        );
        assert!(Cli::try_parse_from(["zirc", "post", "--org", "hi"]).is_err());
    }

    #[test]
    fn helps_however_it_is_asked() {
        use clap::CommandFactory as _;
        use clap::error::ErrorKind;
        Cli::command().debug_assert();
        for args in [
            &["zirc"][..],
            &["zirc", "--help"],
            &["zirc", "-h"],
            &["zirc", "-?"],
            &["zirc", "daemon", "-?"],
            &["zirc", "repo", "add", "-?"],
        ] {
            let err = Cli::try_parse_from(args).unwrap_err();
            assert!(
                matches!(
                    err.kind(),
                    ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                ),
                "{args:?}: {err}"
            );
        }
        let help = Cli::try_parse_from(["zirc", "-?"]).unwrap_err().to_string();
        assert!(help.contains("Getting started:\n  zirc login"));
    }

    #[test]
    fn help_lists_the_commands_by_what_they_are_for() {
        let help = Cli::try_parse_from(["zirc", "--help"])
            .unwrap_err()
            .to_string();
        let start = help.find("Start:\n  login").expect(&help);
        let say = help.find("Say:\n  post").expect(&help);
        assert!(start < say, "{help}");
        assert!(help.contains("More commands: zirc help --all"), "{help}");
        // What scripts call, and what another command now covers, isn't in the way.
        for hidden in ["  clashes ", "  leases ", "  gotchas ", "  task "] {
            assert!(!help.contains(hidden), "{hidden} in {help}");
        }
        // But it's all there to find, and still runs.
        let all = listed(true);
        assert!(
            all.contains("More:\n") && all.contains("  clashes "),
            "{all}"
        );
        for args in [
            &["zirc", "clashes", "src"][..],
            &["zirc", "leases"],
            &["zirc", "gotchas"],
            &["zirc", "orgs", "remove", "acme"],
            &["zirc", "who", "--owns", "src"],
            &["zirc", "wait-for", "1299"],
        ] {
            Cli::try_parse_from(args).unwrap_or_else(|err| panic!("{args:?}: {err}"));
        }
    }

    /// `line`'s words, as a shell would give them: quotes hold a phrase together.
    fn words_of(line: &str) -> Vec<String> {
        let mut words = vec![String::new()];
        let mut quoted = false;
        for c in line.chars() {
            match c {
                '"' => quoted = !quoted,
                ' ' if !quoted => words.push(String::new()),
                c => words.last_mut().unwrap().push(c),
            }
        }
        words.retain(|word| !word.is_empty());
        words
    }

    #[test]
    fn every_listed_command_has_examples_that_run_as_written() {
        for (_, names) in GROUPS {
            for name in *names {
                let has = EXAMPLES
                    .iter()
                    .any(|(command, lines)| command == name && !lines.is_empty());
                assert!(has, "{name} has no example");
            }
        }
        for (command, lines) in EXAMPLES {
            for line in *lines {
                let words = words_of(line);
                assert_eq!(words[..2], ["zirc", *command], "{line}");
                Cli::try_parse_from(&words).unwrap_or_else(|err| panic!("{line}: {err}"));
            }
            let help = Cli::try_parse_from(["zirc", command, "--help"])
                .unwrap_err()
                .to_string();
            assert!(
                help.contains(&format!("Examples:\n  {}", lines[0])),
                "{help}"
            );
        }
    }

    #[test]
    fn every_command_is_in_one_group_or_hidden() {
        let commands = commands();
        let grouped: Vec<&str> = GROUPS
            .iter()
            .flat_map(|(_, names)| names.iter().copied())
            .collect();
        for command in commands.get_subcommands() {
            let name = command.get_name();
            let times = grouped.iter().filter(|it| **it == name).count();
            let wanted = usize::from(!command.is_hide_set());
            assert_eq!(times, wanted, "{name} is in {times} groups");
        }
        for name in grouped {
            assert!(
                commands.find_subcommand(name).is_some(),
                "{name} isn't a command"
            );
        }
    }

    #[test]
    fn zirc_help_shows_a_commands_help_and_every_command() {
        let shown = |args: &[&str]| {
            let Command::Help { all, command } = Cli::try_parse_from(args).unwrap().command else {
                panic!("{args:?} isn't help");
            };
            let mut out = Vec::new();
            help(all, &command, &mut out).map(|()| String::from_utf8(out).unwrap())
        };
        let post = shown(&["zirc", "help", "post"]).unwrap();
        assert!(post.contains("Usage: zirc post"), "{post}");
        let add = shown(&["zirc", "help", "repo", "add"]).unwrap();
        assert!(add.contains("Usage: zirc repo add"), "{add}");
        assert!(shown(&["zirc", "help"]).unwrap().contains("Start:\n"));
        assert!(
            shown(&["zirc", "help", "--all"])
                .unwrap()
                .contains("More:\n")
        );
        let err = shown(&["zirc", "help", "nope"]).unwrap_err().to_string();
        assert!(err.contains("no command nope"), "{err}");
    }

    #[test]
    fn the_merged_commands_take_what_the_old_ones_did() {
        use clap::CommandFactory as _;
        for args in [
            &["zirc", "lease"][..],
            &["zirc", "lease", "--json"],
            &["zirc", "lease", "build", "--for", "20m"],
            &["zirc", "gotcha"],
            &["zirc", "gotcha", "src", "--json"],
            &["zirc", "gotcha", "src", "-m", "split", "it"],
            &[
                "zirc",
                "gotcha",
                "--retire",
                "01jbx7ab",
                "--session",
                "cc-1",
            ],
            &["zirc", "who", "src/login", "--session", "cc-1"],
            &["zirc", "logout", "acme"],
        ] {
            Cli::try_parse_from(args).unwrap_or_else(|err| panic!("{args:?}: {err}"));
        }
        // What agents guessed, a dozen times in two days, runs what they meant.
        for (guess, means) in [("read", "catchup"), ("whoami", "status")] {
            let parsed = Cli::command()
                .try_get_matches_from(["zirc", guess])
                .unwrap_or_else(|err| panic!("{guess}: {err}"));
            assert_eq!(parsed.subcommand_name(), Some(means), "{guess}");
        }
        // Listing and keeping don't mix, nor a path with the old --owns.
        assert!(Cli::try_parse_from(["zirc", "gotcha", "src", "-m", "x", "--json"]).is_err());
        assert!(Cli::try_parse_from(["zirc", "gotcha", "src", "--retire", "01jbx7ab"]).is_err());
        assert!(Cli::try_parse_from(["zirc", "who", "src", "--owns", "src"]).is_err());
    }

    #[test]
    fn a_reader_that_stopped_reading_is_no_failure() {
        let closed = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
        assert!(stopped_reading(&closed.context("printing the status")));
        assert!(!stopped_reading(&anyhow::anyhow!("zircd isn't running")));
    }

    #[test]
    fn summarises_each_command_in_a_line() {
        use clap::CommandFactory as _;
        let mut commands = vec![Cli::command()];
        while let Some(command) = commands.pop() {
            for sub in command.get_subcommands() {
                let about = sub.get_about().map(ToString::to_string).unwrap_or_default();
                assert!(!about.is_empty(), "{} has no summary", sub.get_name());
                assert!(
                    about.len() <= 80,
                    "{}'s summary is too long: {about}",
                    sub.get_name()
                );
                commands.push(sub.clone());
            }
        }
    }
}
