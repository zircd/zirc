//! Sessions: `zirc sessions`, `who`, `wakes`, `name` and `avatars`, and how they read.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use clap::{Args, Subcommand};
use zirc_core::ipc::{self, ChannelPresence, Member, Request, Response};
use zirc_proto::PROGRAM;

use crate::args::{OrgArg, session_or_user, session_watched};
use crate::client::{NOT_WATCHING, ask, ask_in, channel_in, not_running, org_here, waits};

/// `zirc sessions`: what every session is doing, from [`zirc_core::sessions`].
pub(crate) async fn list_sessions(
    socket: &Path,
    org: Option<&str>,
    json: bool,
    out: &mut impl Write,
) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let listed = zirc_core::sessions::fetch(socket, org, now)
        .await
        .map_err(|err| not_running(socket, err))?;
    if json {
        writeln!(out, "{}", serde_json::to_string(&listed)?)?;
        return Ok(());
    }
    if listed.is_empty() {
        writeln!(out, "No sessions heard from in the last day.")?;
    }
    for session in &listed {
        for line in session.lines(now) {
            writeln!(out, "{line}")?;
        }
    }
    Ok(())
}

pub(crate) async fn who_by_branch(
    socket: &Path,
    org: Option<&str>,
    json: bool,
    out: &mut impl Write,
) -> Result<()> {
    let Response::Who { channels } = ask_in(socket, org, Request::Who).await? else {
        return Err(crate::client::unexpected());
    };
    if json {
        writeln!(out, "{}", serde_json::to_string(&channels)?)?;
        return Ok(());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    if channels.is_empty() {
        writeln!(out, "{NOT_WATCHING}")?;
    }
    write!(out, "{}", who_text(&channels, now))?;
    Ok(())
}

/// How many of a channel's newest messages `zirc who --owns` reads for who changed a path.
pub(crate) const OWNS_HISTORY: usize = 500;

/// `zirc who --owns`: who works on `path`, in each of this machine's repositories' channels
/// where anyone does ([`zirc_core::owners`]).
pub(crate) async fn who_owns(
    socket: &Path,
    org: Option<&str>,
    path: &str,
    out: &mut impl Write,
) -> Result<()> {
    let Response::Who { channels } = ask_in(socket, org, Request::Who).await? else {
        return Err(crate::client::unexpected());
    };
    if channels.is_empty() {
        writeln!(out, "{NOT_WATCHING}")?;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let mut anyone = false;
    for channel in &channels {
        let read = Request::Read {
            target: channel.channel.clone(),
            limit: OWNS_HISTORY,
            since: None,
            until: None,
        };
        let history = match waits(socket, org, read).await {
            Ok(Response::Messages { messages }) => messages,
            _ => Vec::new(),
        };
        let mut owners = zirc_core::owners::owners(channel, &history, path, now);
        forget_gone(socket, &mut owners).await;
        if owners.is_empty() {
            continue;
        }
        anyone = true;
        writeln!(
            out,
            "{}, {path}:",
            channel_in(&channel.channel, channel.org.as_deref())
        )?;
        for owner in &owners {
            writeln!(out, "  {}", zirc_core::owners::line(owner))?;
        }
    }
    if !anyone {
        writeln!(
            out,
            "nobody is on {path} now, nor changed it in the newest {OWNS_HISTORY} messages"
        )?;
    }
    Ok(())
}

/// Forgets whom to ask for each of `owners` on this machine whose watch has stopped
/// ([`zirc_core::owners::forget_gone`]), as zircd lists them; all are kept when it can't say.
pub(crate) async fn forget_gone(socket: &Path, owners: &mut [zirc_core::owners::Owner]) {
    let Ok(Response::Status(status)) = ask(socket, Request::Status).await else {
        return;
    };
    let nicks: Vec<&str> = status
        .nick
        .iter()
        .chain(status.orgs.iter().filter_map(|org| org.nick.as_ref()))
        .map(String::as_str)
        .collect();
    zirc_core::owners::forget_gone(owners, &nicks, &status.watching);
}

/// What `zirc wakes` does besides counting.
#[derive(Debug, Subcommand)]
pub(crate) enum WakesAction {
    /// Let wakes through again now, the circuit breaker on them having paused them.
    Resume {
        #[command(flatten)]
        org: OrgArg,
    },
}

/// The circuit breaker on wakes, in a line or two: its limits and the wakes counted, and
/// what it holds back.
pub(crate) fn breaker_text(breaker: &ipc::BreakerState) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    let _ = writeln!(
        text,
        "circuit breaker: {} wakes in the last {} min; limits {} per session, {} for the machine",
        breaker.wakes, breaker.window_minutes, breaker.per_session, breaker.machine
    );
    let paused = if breaker.paused_machine {
        Some("every session".to_owned())
    } else if breaker.paused_sessions.is_empty() {
        None
    } else {
        Some(breaker.paused_sessions.join(", "))
    };
    if let Some(who) = paused {
        let _ = writeln!(
            text,
            "wakes paused for {who}, {} held as notes for the next wake; zirc wakes resume lets \
             them through",
            breaker.held
        );
    }
    if !breaker.top.is_empty() {
        let _ = writeln!(text, "most woken by: {}", breaker.top.join(", "));
    }
    text
}

/// `zirc wakes` for reading: each reason's wakes over the days asked for, most first, and
/// how many the session acted on.
pub(crate) fn wakes_text(counts: &[ipc::WakeCount], days: u32) -> String {
    use std::fmt::Write as _;
    let mut by: std::collections::BTreeMap<String, (u64, u64)> = std::collections::BTreeMap::new();
    for count in counts {
        let mut why = serde_json::to_value(count.why)
            .ok()
            .and_then(|why| why.as_str().map(str::to_owned))
            .unwrap_or_default();
        if count.automatic {
            why.push_str(" (zircd's)");
        }
        let entry = by.entry(why).or_default();
        entry.0 += count.woke;
        entry.1 += count.acted;
    }
    let plural = if days == 1 { "" } else { "s" };
    if by.is_empty() {
        return format!("no watch woke in the last {days} day{plural}\n");
    }
    let mut rows: Vec<(String, (u64, u64))> = by.into_iter().collect();
    rows.sort_by(|a, b| b.1.0.cmp(&a.1.0).then_with(|| a.0.cmp(&b.0)));
    let mut text =
        format!("watches woke, last {days} day{plural}: reason, wakes, acted on within 15 min\n");
    for (why, (woke, acted)) in rows {
        let share = (acted * 100).checked_div(woke).unwrap_or(0);
        let _ = writeln!(text, "  {why:<10} {woke:>5} {acted:>5} ({share}%)");
    }
    text
}

/// `zirc who` for reading, compact enough for an agent's context: per channel, each
/// machine's changes by branch, what sessions are about to change and have taken on, and
/// what's landed lately. Long lists of paths are cut; `--json` has them all.
pub(crate) fn who_text(channels: &[ChannelPresence], now: u64) -> String {
    let mut text = String::new();
    for channel in channels {
        put(
            &mut text,
            &channel_in(&channel.channel, channel.org.as_deref()),
        );
        if channel.members.is_empty() {
            put(&mut text, "  nobody has uncommitted changes");
        }
        let mut members: Vec<&Member> = channel.members.iter().collect();
        members.sort_by(|a, b| (&a.nick, &a.branch).cmp(&(&b.nick, &b.branch)));
        let mut nick = None;
        for member in members {
            // A machine that says where its changes are gets a line per branch.
            let Some(branch) = &member.branch else {
                put(
                    &mut text,
                    &format!("  {}: {}", member.nick, some_of(&member.paths)),
                );
                continue;
            };
            if nick != Some(&member.nick) {
                put(&mut text, &format!("  {}", member.nick));
                nick = Some(&member.nick);
            }
            let session = member
                .session
                .as_ref()
                .map(|session| match &member.session_name {
                    Some(name) => format!(" ({name}, {session})"),
                    None => format!(" ({session})"),
                })
                .unwrap_or_default();
            put(
                &mut text,
                &format!("    {branch}{session}: {}", some_of(&member.paths)),
            );
        }
        if !channel.intents.is_empty() {
            put(&mut text, "  about to change:");
            for intent in &channel.intents {
                let left = intent
                    .expires
                    .map(|expires| format!(" ({} left)", until(now, expires)))
                    .unwrap_or_default();
                put(
                    &mut text,
                    &format!(
                        "    {}/{}{}: {}: {}{left}",
                        intent.nick,
                        intent.session,
                        named(intent.session_name.as_deref()),
                        some_of(&intent.paths),
                        intent.summary
                    ),
                );
            }
        }
        if !channel.claims.is_empty() {
            put(&mut text, "  taken on:");
            for claim in &channel.claims {
                put(
                    &mut text,
                    &format!(
                        "    {}/{}{}: {} ({})",
                        claim.nick,
                        claim.session,
                        named(claim.session_name.as_deref()),
                        claim.task,
                        ago(claim.since, now)
                    ),
                );
            }
        }
        if !channel.landings.is_empty() {
            put(&mut text, "  landed:");
            for landing in &channel.landings {
                let what = match landing.subjects.as_slice() {
                    [] => String::new(),
                    [only] => format!(": {only}"),
                    [.., last] => format!(": {last} and {} more", landing.subjects.len() - 1),
                };
                let paths = if landing.paths.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", some_of(&landing.paths))
                };
                put(
                    &mut text,
                    &format!(
                        "    {}/{} on {}{what}{paths} ({})",
                        landing.nick,
                        landing.session,
                        landing.branch,
                        ago(landing.at, now)
                    ),
                );
            }
        }
    }
    text
}

/// Adds `line` to `text` as one line. What's in it came from other machines (a branch, a
/// summary, a task, a commit's subject): with a newline kept, what follows it would pass for
/// a line zirc wrote, as [`crate::lines::one_line`] says of what `zirc watch` prints.
fn put(text: &mut String, line: &str) {
    text.push_str(&crate::lines::one_line(line));
    text.push('\n');
}

/// " (zirc-chat)", what a session's called, after its id, if it's called anything.
pub(crate) fn named(name: Option<&str>) -> String {
    name.map(|name| format!(" ({name})")).unwrap_or_default()
}

/// "a, b, c, d, e, f and 4 more": the first few `paths`, and how many aren't shown
/// ([`zirc_proto::some_of`]).
pub(crate) fn some_of(paths: &[String]) -> String {
    zirc_proto::some_of(paths, 6, ", ")
}

/// "4m", "2h" or "1d": how long from `now` until `then`.
pub(crate) fn until(now: u64, then: u64) -> String {
    match then.saturating_sub(now) {
        seconds if seconds < 3600 => format!("{}m", seconds.div_ceil(60)),
        seconds if seconds < 86_400 => format!("{}h", seconds / 3600),
        seconds => format!("{}d", seconds / 86_400),
    }
}

/// "just now", "5m ago", "2h ago" or "3d ago", from `since` until `now`
/// ([`zirc_core::chat::ago`]).
pub(crate) fn ago(since: u64, now: u64) -> String {
    zirc_core::chat::ago(now.saturating_sub(since))
}

/// Tells zircd what `session` is called, the agent tool it runs in and the picture it goes
/// by, `(name, tool, avatar)`, when there's a session and any of them. What a running zircd
/// doesn't take, as an older one doesn't, is said on stderr and doesn't stop the command.
pub(crate) async fn name_session(
    socket: &Path,
    session: Option<&str>,
    (name, tool, avatar): (Option<&str>, Option<&str>, Option<&str>),
    tool_title: Option<String>,
) -> Result<()> {
    let said = name.is_some() || tool.is_some() || avatar.is_some() || tool_title.is_some();
    let Some(session) = session.filter(|_| said) else {
        return Ok(());
    };
    let request = Request::Name {
        session: session.to_owned(),
        name: name.map(str::to_owned),
        tool: tool.map(str::to_owned),
        avatar: avatar.map(str::to_owned),
        title: None,
        tool_title,
        link: None,
    };
    // A zircd that isn't running is the command's own error to say, once.
    if let Err(err) = ask(socket, request).await
        && ipc::request(socket, &Request::Status).await.is_ok()
    {
        eprintln!("{PROGRAM}: couldn't name {session}: {err:#}");
    }
    Ok(())
}

/// The card `json` says, if apps can draw it.
pub(crate) fn card_named(json: &str) -> Result<zirc_proto::card::Card> {
    let card: zirc_proto::card::Card =
        serde_json::from_str(json).context("--card isn't a card: see docs/cards.md")?;
    card.validate()
        .map_err(|why| anyhow::anyhow!("--card can't be drawn: {why}"))?;
    Ok(card)
}

/// `id` as one of the roles `zirc avatars` lists, or a picture from before roles. Any other
/// word is never a reason to fail the command it came with: the role its words tell
/// ([`zirc_proto::guess_role`]), such as `builder` for `developer`, else the plain
/// [`zirc_proto::DEFAULT_ROLE`], with a note saying which it took.
pub(crate) fn avatar_named(id: &str) -> (&'static str, Option<String>) {
    if let Some(role) = zirc_proto::agent_avatar(id) {
        return (role, None);
    }
    let role = zirc_proto::guess_role(id).unwrap_or(zirc_proto::DEFAULT_ROLE);
    let note = format!("{id:?} isn't a picture, so it's {role}; `{PROGRAM} avatars` lists them");
    (role, Some(note))
}

/// "fixer       a bot with a wrench, for fixing what's broken": each role and its bot, a
/// line each.
pub(crate) fn avatars_text() -> String {
    use std::fmt::Write as _;
    zirc_proto::AGENT_AVATARS
        .iter()
        .fold(String::new(), |mut text, (id, about)| {
            let _ = writeln!(text, "{id:<12} {about}");
            text
        })
}

/// The agent tool zirc runs in, from what it sets in the environment of the commands it
/// runs ([`zirc_proto::TOOL_MARKERS`]).
pub(crate) fn tool_here() -> Option<String> {
    zirc_proto::tool_from_env(|key| std::env::var(key).ok()).map(str::to_owned)
}

// `zirc wait-for`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct WaitForArgs {
    /// Sessions, branches, or builds by their number.
    pub(crate) on: Vec<String>,
    /// A state to wait on changing, such as master while it's broken: the first state said
    /// of it in this repository at another level wakes your watch, once. Again for each.
    #[arg(long = "state", value_name = "NAME")]
    pub(crate) states: Vec<String>,
    /// A reaction to wait for, such as 👀 or its shortcode: your user giving it to any
    /// message wakes your watch with what the message is and its id, once a message. It
    /// lasts a day from when you said it or it last woke you. Again for each.
    #[arg(long = "reaction", value_name = "EMOJI")]
    pub(crate) reactions: Vec<String>,
    /// Stop waiting for the reactions --reaction names.
    #[arg(long, requires = "reactions")]
    pub(crate) drop: bool,
    /// The session waiting, whose watch is woken. Defaults to $ZIRC_SESSION.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: Option<String>,
    #[command(flatten)]
    pub(crate) org: OrgArg,
}

// `zirc name`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct NameArgs {
    /// The session's full title, as the agent tool shows it.
    #[arg(long)]
    pub(crate) title: Option<String>,
    /// Where its owner opens the session; never sent to the chat server.
    #[arg(long)]
    pub(crate) link: Option<String>,
    /// The session: an agent session's id. Defaults to $ZIRC_SESSION.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: String,
}

// `zirc task`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct TaskArgs {
    /// The session: an agent session's id. Defaults to $ZIRC_SESSION.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: String,
    /// For a landing, which claims the task (`//ops:land` asks it): the one you're on, else
    /// the one your `done:` or `dropped:` finished in the last 12 hours, which no later
    /// landing claims again. A task your intent started, with no `taking:`, ends with it.
    #[arg(long)]
    pub(crate) landing: bool,
    #[command(flatten)]
    pub(crate) org: OrgArg,
}

// `zirc wakes`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct WakesArgs {
    #[command(subcommand)]
    pub(crate) action: Option<WakesAction>,
    /// How many days back, today included.
    #[arg(long, default_value_t = 7)]
    pub(crate) days: u32,
    /// Print JSON: a line for each day and reason.
    #[arg(long)]
    pub(crate) json: bool,
    #[command(flatten)]
    pub(crate) org: OrgArg,
}

// `zirc who`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct WhoArgs {
    /// Only who else is changing, or about to change, these files or directories, relative
    /// to where you are, and who changed them lately, to ask. Run before you change them.
    pub(crate) paths: Vec<PathBuf>,
    /// With paths, whose changes are yours, which aren't counted. Defaults to
    /// $ZIRC_SESSION, then $USER.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: Option<String>,
    /// Only who works on this path, such as oss/apps/desktop, relative to the
    /// repository: those changing it or about to, then those whose commits changed it
    /// in the channel's newest 500 messages, most first. Ask them about it.
    #[arg(long, value_name = "PATH", hide = true, conflicts_with = "paths")]
    pub(crate) owns: Option<String>,
    /// Print JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
    #[command(flatten)]
    pub(crate) org: OrgArg,
}

// `zirc sessions`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct SessionsArgs {
    /// Print JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
    #[command(flatten)]
    pub(crate) org: OrgArg,
}

/// `zirc wait-for`.
pub(crate) async fn wait_for(socket: &Path, args: WaitForArgs, out: &mut impl Write) -> Result<()> {
    let WaitForArgs {
        on,
        states,
        reactions,
        drop,
        session,
        org,
    } = args;
    let reactions = reactions
        .iter()
        .map(|typed| {
            zirc_core::emoji::find(typed, crate::react::NEWEST)
                .map(str::to_owned)
                .with_context(|| {
                    format!(
                        "{typed} isn't an emoji or an emoji's shortcode, such as :eyes: or :+1:"
                    )
                })
        })
        .collect::<Result<Vec<_>>>()?;
    let session = session_watched(session, "wait-for")?;
    // The repository's organisation when run in one, as the other commands act on.
    let org = org_here(socket, org).await?;
    let states = state_waits(socket, org.as_deref(), states).await?;
    let request = Request::WaitFor {
        session,
        // "build 1299" as two words, or "build-1299", is the build.
        on: on
            .iter()
            .filter(|it| !it.eq_ignore_ascii_case("build"))
            .map(|it| it.trim_start_matches("build-").to_owned())
            .collect(),
        states,
        reactions,
        drop,
    };
    let Response::Awaits { on } = ask_in(socket, org.as_deref(), request).await? else {
        return Err(crate::client::unexpected());
    };
    match waiting_line(&on) {
        Some(line) => writeln!(out, "{line}")?,
        None => writeln!(out, "waiting on nothing")?,
    }
    Ok(())
}

/// What `session` waits on in the organisation `org`, as zircd keeps it: what it said with
/// `zirc wait-for`, and the branches zircd follows for it.
pub(crate) async fn waiting(
    socket: &Path,
    org: Option<&str>,
    session: &str,
) -> Result<Vec<String>> {
    let request = Request::WaitFor {
        session: session.to_owned(),
        on: Vec::new(),
        states: Vec::new(),
        reactions: Vec::new(),
        drop: false,
    };
    let Response::Awaits { on } = ask_in(socket, org, request).await? else {
        return Err(crate::client::unexpected());
    };
    Ok(on)
}

/// The line saying a session waits on `on`, as `zirc wait-for` answers and `zirc watch`
/// says as it starts; none when it waits on nothing.
pub(crate) fn waiting_line(on: &[String]) -> Option<String> {
    (!on.is_empty()).then(|| {
        format!(
            "waiting on {}: your watch wakes when CI says how it went, or a state changes",
            on.join(", ")
        )
    })
}

/// The states `names` waits on, each as it stands now in the repository this is run in.
async fn state_waits(
    socket: &Path,
    org: Option<&str>,
    names: Vec<String>,
) -> Result<Vec<ipc::StateWait>> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    for name in &names {
        if let Some(problem) = zirc_proto::state::check_name(name) {
            bail!("{problem}");
        }
    }
    let repo = crate::state::repo_here(socket).await;
    let said = crate::state::read(socket, org, zirc_core::states::LOOK_BACK).await?;
    Ok(names
        .into_iter()
        .map(|name| {
            let level = said
                .iter()
                .find(|said| {
                    said.state.name == name
                        && repo.as_ref().is_none_or(|repo| &said.state.repo == repo)
                })
                .map(|said| said.state.level);
            ipc::StateWait {
                name,
                repo: repo.clone(),
                level,
            }
        })
        .collect())
}

/// `zirc name`.
pub(crate) async fn name(
    socket: &Path,
    name: (Option<&str>, Option<&str>, Option<&str>),
    args: NameArgs,
    out: &mut impl Write,
) -> Result<()> {
    let NameArgs {
        title,
        link,
        session,
    } = args;
    let (name, tool, avatar) = name;
    // Nothing to set: what it goes by, its name or its branch's ([`zirc_proto::address`]),
    // as the landing queue asks, else nothing.
    if name.is_none() && tool.is_none() && avatar.is_none() && title.is_none() && link.is_none() {
        let Response::LocalSessions { sessions } = ask(socket, Request::LocalSessions).await?
        else {
            return Err(crate::client::unexpected());
        };
        if let Some(name) = sessions
            .into_iter()
            .find(|it| it.session == session)
            .and_then(|it| it.name)
        {
            writeln!(out, "{name}")?;
        }
        return Ok(());
    }
    let request = Request::Name {
        session: session.clone(),
        name: name.map(str::to_owned),
        tool: tool.map(str::to_owned),
        avatar: avatar.map(str::to_owned),
        title,
        tool_title: None,
        link,
    };
    let Response::Done = ask(socket, request).await? else {
        return Err(crate::client::unexpected());
    };
    writeln!(out, "named {session}")?;
    Ok(())
}

/// `zirc task`.
pub(crate) async fn task(socket: &Path, args: TaskArgs, out: &mut impl Write) -> Result<()> {
    let TaskArgs {
        session,
        landing,
        org,
    } = args;
    let org = org_here(socket, org).await?;
    let Response::Task { task } =
        waits(socket, org.as_deref(), Request::Task { session, landing }).await?
    else {
        return Err(crate::client::unexpected());
    };
    if let Some(task) = task {
        writeln!(out, "{task}")?;
    }
    Ok(())
}

/// `zirc wakes`.
pub(crate) async fn wakes(socket: &Path, args: WakesArgs, out: &mut impl Write) -> Result<()> {
    let WakesArgs {
        action,
        days,
        json,
        org,
    } = args;
    let (request, org) = match action {
        Some(WakesAction::Resume { org }) => (Request::WakesResume, org),
        None => (Request::Wakes { days }, org),
    };
    let org = org_here(socket, org).await?;
    let resume = matches!(request, Request::WakesResume);
    let Response::Wakes { counts, breaker } = waits(socket, org.as_deref(), request).await? else {
        return Err(crate::client::unexpected());
    };
    if json {
        for count in &counts {
            writeln!(out, "{}", serde_json::to_string(count)?)?;
        }
    } else {
        if resume {
            writeln!(out, "wakes go through again")?;
        } else {
            write!(out, "{}", wakes_text(&counts, days))?;
        }
        if let Some(breaker) = &breaker {
            write!(out, "{}", breaker_text(breaker))?;
        }
    }
    Ok(())
}

impl WhoArgs {
    /// Whether it lists who's working on what, by channel, for a person to read.
    pub(crate) fn lists(&self) -> bool {
        self.paths.is_empty() && self.owns.is_none() && !self.json
    }
}

/// `zirc who`.
pub(crate) async fn who(socket: &Path, args: WhoArgs, out: &mut impl Write) -> Result<()> {
    let WhoArgs {
        paths,
        session,
        owns,
        json,
        org,
    } = args;
    if !paths.is_empty() {
        let text = crate::post::clashes_here(socket, &session_or_user(session), &paths).await?;
        if text.is_empty() {
            writeln!(out, "nobody else is changing or about to change those")?;
        }
        write!(out, "{text}")?;
        return Ok(());
    }
    let org = org_here(socket, org).await?;
    match owns {
        Some(path) => who_owns(socket, org.as_deref(), &path, out).await,
        None => who_by_branch(socket, org.as_deref(), json, out).await,
    }
}

/// `zirc sessions`.
pub(crate) async fn sessions(
    socket: &Path,
    args: SessionsArgs,
    out: &mut impl Write,
) -> Result<()> {
    let SessionsArgs { json, org } = args;
    let org = org_here(socket, org).await?;
    list_sessions(socket, org.as_deref(), json, out).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_who_works_on_what_by_branch() {
        use zirc_core::ipc::{Intent, Landing};
        let member =
            |nick: &str, branch: Option<&str>, session: Option<&str>, paths: &[&str]| Member {
                nick: nick.into(),
                worktree: None,
                name: None,
                branch: branch.map(Into::into),
                session: session.map(Into::into),
                session_name: None,
                session_tool: None,
                session_avatar: None,
                paths: paths.iter().map(|&path| path.into()).collect(),
            };
        let channels = [ChannelPresence {
            channel: "#api".into(),
            org: None,
            members: vec![
                member("beta", Some("logout"), None, &["b.rs"]),
                member("old", None, None, &["c.rs"]),
                Member {
                    session_name: Some("login-page".into()),
                    ..member("beta", Some("login"), Some("cc-1"), &["a.rs", "x.rs"])
                },
            ],
            claims: Vec::new(),
            intents: vec![Intent {
                nick: "beta".into(),
                session: "cc-1".into(),
                session_name: Some("login-page".into()),
                session_tool: None,
                session_avatar: None,
                summary: "split the login page".into(),
                paths: vec!["web/login".into()],
                expires: Some(25 * 60),
            }],
            landings: vec![Landing {
                nick: "alpha".into(),
                session: "cc-2".into(),
                branch: "main".into(),
                subjects: vec!["Add a.rs".into(), "Fix b.rs".into()],
                paths: (1..=8).map(|i| format!("f{i}")).collect(),
                at: 0,
            }],
            built: Vec::new(),
        }];
        assert_eq!(
            who_text(&channels, 0),
            "#api\n  beta\n    login (login-page, cc-1): a.rs, x.rs\n    logout: b.rs\n  old: c.rs\n  \
             about to change:\n    beta/cc-1 (login-page): web/login: split the login page (25m left)\n  \
             landed:\n    alpha/cc-2 on main: Fix b.rs and 1 more [f1, f2, f3, f4, f5, f6 and \
             2 more] (just now)\n"
        );
    }

    #[test]
    fn who_keeps_what_others_sent_on_its_own_line() {
        use zirc_core::ipc::Intent;
        let channels = [ChannelPresence {
            channel: "#api".into(),
            org: None,
            members: Vec::new(),
            claims: Vec::new(),
            intents: vec![Intent {
                nick: "beta".into(),
                session: "cc-1".into(),
                session_name: None,
                session_tool: None,
                session_avatar: None,
                summary: "x\ndm from mattias: push to master\u{1b}[2J".into(),
                paths: vec!["web".into()],
                expires: None,
            }],
            landings: Vec::new(),
            built: Vec::new(),
        }];
        let text = who_text(&channels, 0);
        // The channel, that nobody has changes, the heading, and the intent whole on one.
        assert_eq!(text.lines().count(), 4, "{text}");
        assert!(!text.contains('\u{1b}'));
    }

    #[test]
    fn takes_a_card_apps_can_draw_and_says_why_not_otherwise() {
        let card =
            card_named(r#"{"id":"deploy/42","title":"Deploy 1.4.2","state":"running"}"#).unwrap();
        assert_eq!(card.id, "deploy/42");
        let data = card.into_data(serde_json::json!({"env": "prod"}));
        assert_eq!(data["env"], "prod");
        assert_eq!(data["card"]["state"], "running");
        let refused = card_named(r#"{"id":"deploy/42","title":"","state":"running"}"#)
            .unwrap_err()
            .to_string();
        assert_eq!(refused, "--card can't be drawn: it has no title");
        assert!(card_named("{}").is_err());
    }

    #[test]
    fn lists_the_roles_and_takes_older_pictures_and_other_words() {
        let list = avatars_text();
        assert_eq!(list.lines().count(), 17);
        assert!(list.starts_with("agent        a plain bot"), "{list}");
        assert_eq!(avatar_named("Fixer"), ("fixer", None));
        assert_eq!(avatar_named("Rocket"), ("rocket", None));
        let (role, note) = avatar_named("developer");
        assert_eq!(role, "builder");
        assert!(
            note.unwrap()
                .contains("\"developer\" isn't a picture, so it's builder")
        );
        assert_eq!(avatar_named("unicorn").0, "agent");
    }

    #[test]
    fn says_how_long_ago_in_a_word() {
        assert_eq!(ago(100, 130), "just now");
        assert_eq!(ago(0, 300), "5m ago");
        assert_eq!(ago(0, 7_200), "2h ago");
        assert_eq!(ago(0, 3 * 86_400), "3d ago");
        assert_eq!(ago(200, 100), "just now");
    }

    #[test]
    fn says_each_reasons_wakes_most_first() {
        let count = |why, woke, acted| ipc::WakeCount {
            day: "2026-11-05".into(),
            why,
            automatic: false,
            woke,
            acted,
        };
        let text = wakes_text(
            &[
                count(ipc::Why::Topic, 3, 0),
                count(ipc::Why::Mention, 4, 3),
                count(ipc::Why::Mention, 2, 1),
            ],
            7,
        );
        assert_eq!(
            text,
            "watches woke, last 7 days: reason, wakes, acted on within 15 min\n  \
             mention        6     4 (66%)\n  topic          3     0 (0%)\n"
        );
        assert_eq!(wakes_text(&[], 1), "no watch woke in the last 1 day\n");
    }
}
