//! `zirc watch`: what concerns a session, as it comes.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::Args;
use zirc_core::filter::Filter;
use zirc_core::ipc::{self, Delivery, Heard, Request, Response};
use zirc_proto::{Event, Level, PROGRAM};

use crate::client::{ask, not_running};
use crate::daemon::{SocketId, socket_id};
use crate::decide::answered_line;
use crate::hints::{hint_line, watch_hints};
use crate::lines::{
    conflict_line, direct_line, line, local_overlap_line, one_line, overlap_line, reaction_line,
};
use crate::repo::{repo_here, unregistered};
use crate::sessions::{name_session, waiting, waiting_line};
use crate::status::stuck_channelless;

/// How long `zirc watch` waits for a zircd that stopped to be back, as one updating itself
/// is in moments, or one launchd starts again is within ten seconds or so.
pub(crate) const REWATCH_WAIT: Duration = Duration::from_secs(60);

/// How long `zirc watch --once` waits for zircd to be back, quietly: it ending wakes the
/// session for nothing but to start it again, which cost sessions 19 wakes on 5 Nov 2026
/// while zircd was stopped or updated. A watch printing as it goes keeps [`REWATCH_WAIT`],
/// so a tool following its lines soon hears it stopped.
pub(crate) const ONCE_REWATCH_WAIT: Duration = Duration::from_mins(30);

/// How long a watch waits for zircd to be back, as `once` says.
pub(crate) fn rewatch_wait(once: bool) -> Duration {
    if once {
        ONCE_REWATCH_WAIT
    } else {
        REWATCH_WAIT
    }
}

/// What `zirc watch` prints: everything or what concerns the session, as lines or JSON, or
/// what a filter makes of it.
#[derive(Clone, Copy)]
pub(crate) struct How<'a> {
    pub(crate) all: bool,
    pub(crate) json: bool,
    pub(crate) filter: Option<&'a Filter>,
    /// What ends the watch, besides --once and --for: the first event it picks out.
    pub(crate) stop_at: Option<&'a Filter>,
    /// Other sessions whose posts, and posts naming them, are printed too.
    pub(crate) follow: &'a [String],
    /// Channels everything said in is printed too.
    pub(crate) channels: &'a [String],
    /// The least level a line said in `channels` is printed at; what concerns the session
    /// is printed whatever its level.
    pub(crate) level: Option<Level>,
    /// Stop after the first burst of lines.
    pub(crate) once: bool,
    /// The hints for what wakes the session, and which it's seen already.
    pub(crate) hints: &'a std::sync::Mutex<zirc_core::hints::Watch>,
}

/// How long after a line `zirc watch --once` waits for more that come with it, such as the
/// rest of what was missed, before it stops.
pub(crate) const ONCE_BURST: Duration = Duration::from_millis(200);

pub(crate) async fn watch_as(
    socket: &Path,
    how: &How<'_>,
    session: Option<&str>,
    until: Option<tokio::time::Instant>,
    out: &mut impl Write,
) -> Result<()> {
    let mut watching = ipc::watch(socket, session)
        .await
        .map_err(|err| not_running(socket, err))?;
    in_channels(socket, how.channels, session).await?;
    for line in starting(socket, session).await {
        aside(how.once, out, &line)?;
    }
    let mut watched = socket_id(socket);
    let ending = async {
        match until {
            Some(until) => tokio::time::sleep_until(until).await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(ending);
    // Once --once has printed, when the burst it's printing ends.
    let mut burst: Option<tokio::time::Instant> = None;
    loop {
        let bursting = async {
            match burst {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        let next = tokio::select! {
            next = watching.next() => next,
            // Quietly: a line would wake the session to say nothing.
            () = &mut ending => return Ok(()),
            () = bursting => return Ok(()),
            () = orphaned() => return Ok(()),
        };
        let heard = match next {
            Ok(Some(heard)) => Some(heard),
            // As after an update, or a zircd that stopped and is started again, as launchd
            // or a session does: watch the new one once it's there.
            Ok(None) | Err(ipc::Error::Io(_)) => None,
            Err(ipc::Error::Daemon(message)) if message == ipc::RESTARTING => None,
            Err(err) => return Err(err.into()),
        };
        let Some(heard) = heard else {
            watching = rewatch(socket, watched, session, rewatch_wait(how.once)).await?;
            watched = socket_id(socket);
            aside(how.once, out, "zircd restarted; watching again")?;
            continue;
        };
        let stop = picks(how.stop_at, &heard);
        if stop {
            // Printed however it would be, and the watch ends with the burst it's in.
            print_heard(heard, &How { all: true, ..*how }, out)?;
            burst.get_or_insert(tokio::time::Instant::now() + ONCE_BURST);
        } else if print_heard(heard, how, out)? && how.once {
            burst = Some(tokio::time::Instant::now() + ONCE_BURST);
        }
        record_hints(socket, how, session).await;
    }
}

/// Tells the recorder of each hint the watch just showed `session`, by its id.
async fn record_hints(socket: &Path, how: &How<'_>, session: Option<&str>) {
    let shown = how
        .hints
        .lock()
        .map(|mut hints| hints.take_shown())
        .unwrap_or_default();
    let Some(session) = session else {
        return;
    };
    for shown in shown {
        let record = zirc_proto::activity::HintShown {
            hint: shown.id,
            kind: shown.kind,
            layer: shown.layer.name().to_owned(),
        };
        crate::playbook::used(socket, session, &record).await;
    }
}

/// Ends once this process is an orphan, its parent gone and itself handed to `init` or
/// launchd (pid 1): the session that started the watch has ended, and nobody reads it.
pub(crate) async fn orphaned() {
    let mut every = tokio::time::interval(Duration::from_secs(30));
    loop {
        every.tick().await;
        if is_orphan(std::os::unix::process::parent_id()) {
            return;
        }
    }
}

/// Whether a process whose parent is `parent` was orphaned.
pub(crate) fn is_orphan(parent: u32) -> bool {
    parent == 1
}

/// What a watch says once, at the start: that the session isn't registered, or that this
/// machine is in no channels, either of which makes a watch look quiet; and what the
/// session waits on, which zircd keeps and no process shows otherwise.
pub(crate) async fn starting(socket: &Path, session: Option<&str>) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(session) = session
        && let Ok(Response::Repos { repos }) = ask(socket, Request::RepoList).await
    {
        let cwd = std::env::current_dir().unwrap_or_default();
        if !repos
            .iter()
            .any(|repo| repo.sessions.iter().any(|it| it.session == session))
        {
            // Said where the session will read it: what it's missing and the fix.
            lines.push(unregistered(session, &repos, &cwd));
        }
        // In the organisation `zirc wait-for` run here would ask.
        let org = repo_here(&repos, &cwd).and_then(|repo| repo.org.as_deref());
        if let Ok(on) = waiting(socket, org, session).await {
            lines.extend(waiting_line(&on));
        }
    }
    if let Ok(Response::Status(status)) = ask(socket, Request::Status).await {
        lines.extend(stuck_channelless(socket, &status).await);
    }
    lines
}

/// Prints `line`, which isn't news, to `out`, or with --once (`once`) to stderr: printed
/// there, it would wake the session only for it to start the watch again and be woken
/// again.
pub(crate) fn aside(once: bool, out: &mut impl Write, line: &str) -> Result<()> {
    if once {
        eprintln!("{line}");
    } else {
        writeln!(out, "{line}")?;
        out.flush()?;
    }
    Ok(())
}

/// The jq filter `jq`, given as `flag`, unless it names a kind zirc never sends, which would
/// make it wait forever for nothing.
fn filter_of(jq: &str, flag: &str) -> Result<Filter> {
    if let Some(kind) = zirc_proto::unknown_zirc_kind(jq) {
        bail!(
            "{flag} names {kind}, a kind zirc never sends, so it would never match. zirc's own \
             events go by .message.event.type: {}, such as `.message.event.type == \"push\"`; \
             the rest by .message.event.kind: {}",
            zirc_proto::MACHINE_EVENTS.join(", "),
            zirc_proto::ZIRC_KINDS.join(", ")
        );
    }
    Ok(Filter::new(jq)?)
}

/// Whether `filter` picks out `heard`: it outputs anything but `false` or `null`. A filter
/// that fails on it, as on an event without the fields it reads, picks nothing.
pub(crate) fn picks(filter: Option<&Filter>, heard: &Heard) -> bool {
    let Some(filter) = filter else {
        return false;
    };
    let Ok(json) = heard_json(heard) else {
        return false;
    };
    filter
        .run(&json)
        .is_ok_and(|lines| lines.iter().any(|line| line != "false" && line != "null"))
}

/// Prints `heard` as `how` says, if it's to be printed: whether it was.
pub(crate) fn print_heard(heard: Heard, how: &How<'_>, out: &mut impl Write) -> Result<bool> {
    let &How {
        all,
        json,
        filter,
        follow,
        channels,
        level,
        ..
    } = how;
    if !all && is_noise(&heard) {
        return Ok(false);
    }
    // A filter sees everything and decides for itself.
    if let Some(filter) = filter {
        let mut printed = false;
        match filter.run(&heard_json(&heard)?) {
            Ok(lines) => {
                for line in lines {
                    writeln!(out, "{line}")?;
                    printed = true;
                }
            }
            Err(err) => writeln!(out, "{PROGRAM}: {err}")?,
        }
        out.flush()?;
        return Ok(printed);
    }
    // Lines wake whoever reads them: only what concerns this session, unless asked for
    // everything. JSON has everything, saying why each line is there.
    let heard = in_channel(heard, channels, level);
    if !all && !json && why(&heard).is_none() && !follows(&heard, follow) {
        return Ok(false);
    }
    // With --once, JSON wakes only for what concerns the session, as lines do.
    if json && !all && why(&heard).is_none() && !follows(&heard, follow) && how.once {
        return Ok(false);
    }
    // A hint, only under what wakes the session.
    let hint = why(&heard)
        .is_some()
        .then(|| how.hints.lock().ok()?.new_for(&heard))
        .flatten();
    match (&heard, json) {
        (heard, true) => {
            let mut value = heard_json(heard)?;
            if let (Some(hint), Some(object)) = (&hint, value.as_object_mut()) {
                object.insert("hint".to_owned(), serde_json::to_value(hint)?);
            }
            writeln!(out, "{value}")?;
        }
        (Heard::Conflict(conflict), false) => {
            writeln!(out, "{}", one_line(&conflict_line(conflict)))?;
        }
        (Heard::Message(delivery), false) => writeln!(out, "{}", one_line(&line(delivery)))?,
        (Heard::Direct(direct), false) => {
            writeln!(out, "{}", one_line(&direct_line(direct)))?;
        }
        (Heard::Overlap(overlap), false) => {
            writeln!(out, "{}", one_line(&overlap_line(overlap)))?;
        }
        (Heard::LocalOverlap(overlap), false) => {
            writeln!(out, "{}", one_line(&local_overlap_line(overlap)))?;
        }
        (Heard::Reaction(reaction), false) => {
            writeln!(out, "{}", one_line(&reaction_line(reaction)))?;
        }
        (Heard::Answered(answered), false) => {
            writeln!(out, "{}", one_line(&answered_line(answered)))?;
        }
    }
    if let Some(hint) = hint.filter(|_| !json) {
        writeln!(out, "{}", hint_line(&hint))?;
    }
    out.flush()?;
    Ok(true)
}

/// A watch again, once a restarting zircd is back: on a socket other than `before`, the old
/// zircd's, which may take a connection in its last moments and then drop it.
pub(crate) async fn rewatch(
    socket: &Path,
    before: Option<SocketId>,
    session: Option<&str>,
    wait: Duration,
) -> Result<ipc::Watching> {
    let start = tokio::time::Instant::now();
    let deadline = start + wait;
    loop {
        let again = if socket_id(socket) == before {
            None
        } else {
            ipc::watch(socket, session).await.ok()
        };
        match again {
            Some(watching) => return Ok(watching),
            // Often at first, as an update restarts zircd in moments; then every few seconds.
            None if tokio::time::Instant::now() < deadline => {
                let every = if start.elapsed() < REWATCH_WAIT {
                    Duration::from_millis(100)
                } else {
                    Duration::from_secs(5)
                };
                tokio::time::sleep(every).await;
            }
            None => bail!(
                "zircd stopped, and no zircd started again within {} minutes",
                wait.as_secs() / 60
            ),
        }
    }
}

/// What `zirc watch --json` prints for `heard`, and what `--filter` runs on.
pub(crate) fn heard_json(heard: &Heard) -> Result<serde_json::Value> {
    Ok(match heard {
        Heard::Message(delivery) => serde_json::to_value(delivery)?,
        Heard::Direct(direct) => serde_json::to_value(direct)?,
        Heard::Overlap(overlap) => serde_json::to_value(overlap)?,
        Heard::LocalOverlap(overlap) => serde_json::to_value(overlap)?,
        Heard::Conflict(conflict) => serde_json::to_value(conflict)?,
        Heard::Reaction(reaction) => serde_json::to_value(reaction)?,
        Heard::Answered(answered) => serde_json::to_value(answered)?,
    })
}

/// Working-set updates arrive with every edit anywhere; what they mean for you arrives as
/// overlaps, so by default they're left out.
/// Why zircd says `heard` concerns the session watching, if it does.
pub(crate) fn why(heard: &Heard) -> Option<ipc::Why> {
    match heard {
        Heard::Message(delivery) => delivery.why,
        Heard::Direct(direct) => direct.why,
        Heard::Overlap(overlap) => overlap.why,
        Heard::LocalOverlap(overlap) => overlap.why,
        Heard::Conflict(conflict) => conflict.why,
        Heard::Reaction(reaction) => reaction.why,
        Heard::Answered(answered) => answered.why,
    }
}

/// `heard`, marked as said in one of `channels` when it is, at `level` or above if one is
/// given, and is what people, agents or CI said there rather than one of zircd's events,
/// unless it concerns the session already.
pub(crate) fn in_channel(heard: Heard, channels: &[String], level: Option<Level>) -> Heard {
    match heard {
        Heard::Message(delivery)
            if delivery.why.is_none()
                && channels
                    .iter()
                    .any(|channel| channel.eq_ignore_ascii_case(&delivery.channel))
                && level.is_none_or(|least| delivery.message.level_or_info() >= least)
                && !matches!(
                    delivery.message.event,
                    Event::Working { .. } | Event::Commit { .. } | Event::Push { .. }
                ) =>
        {
            Heard::Message(Delivery {
                why: Some(ipc::Why::Channel),
                ..delivery
            })
        }
        heard => heard,
    }
}

/// Whether this machine is in each of `channels`, as `zirc watch --channel` needs: an error
/// saying how to join the first it isn't in.
pub(crate) async fn in_channels(
    socket: &Path,
    channels: &[String],
    session: Option<&str>,
) -> Result<()> {
    if channels.is_empty() {
        return Ok(());
    }
    let Response::Status(status) = ask(socket, Request::Status).await? else {
        return Err(crate::client::unexpected());
    };
    not_in(&status, channels, session)
}

/// An error naming the first of `channels` this machine isn't in, by `status`, saying how to
/// join it.
pub(crate) fn not_in(
    status: &ipc::Status,
    channels: &[String],
    session: Option<&str>,
) -> Result<()> {
    let joined: Vec<&String> = status
        .channels
        .iter()
        .chain(status.orgs.iter().flat_map(|org| &org.channels))
        .collect();
    let Some(missing) = channels
        .iter()
        .find(|channel| !joined.iter().any(|it| it.eq_ignore_ascii_case(channel)))
    else {
        return Ok(());
    };
    let session = session.map_or_else(String::new, |session| format!(" --session {session}"));
    let join = match missing.trim_start_matches('#').split_once('/') {
        Some((_, topic)) => format!("zirc join{session} {topic}, in that repository"),
        None => format!(
            "zirc join{session} --org {}",
            missing.trim_start_matches('#')
        ),
    };
    bail!("this machine isn't in {missing}: {join} joins it")
}

/// Whether `heard` is a post by one of the sessions `followed`, or one that names one.
pub(crate) fn follows(heard: &Heard, followed: &[String]) -> bool {
    let Heard::Message(delivery) = heard else {
        return false;
    };
    let text = delivery.message.event.human_text();
    followed.iter().any(|session| {
        delivery.message.session == *session || zirc_core::names::mentions(&text, session)
    })
}

pub(crate) fn is_noise(heard: &Heard) -> bool {
    matches!(heard, Heard::Message(delivery) if matches!(delivery.message.event, Event::Working { .. }))
}

// `zirc watch`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct WatchArgs {
    /// Print everything, whether it concerns you or not, working-set updates included,
    /// which come with each edit anywhere.
    #[arg(long)]
    pub(crate) all: bool,
    /// Print JSON: a line for everything but working-set updates, each with `why` it
    /// concerns you, or null.
    #[arg(long)]
    pub(crate) json: bool,
    /// The session watching: what concerns it is judged by the repositories added with
    /// its name, and messages for this machine's other sessions are left out. Defaults
    /// to $ZIRC_SESSION.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: Option<String>,
    /// A jq filter, run on the JSON of everything heard, whether it concerns you or
    /// not, working-set updates only with --all: what it outputs is printed, a line of
    /// JSON each. Such as: --filter 'select(.message.event.kind == "x-acme/deploy")'.
    #[arg(long, value_name = "JQ", conflicts_with = "json")]
    pub(crate) filter: Option<String>,
    /// Stop after this long, such as 30m or 2h, quietly: start it again then.
    #[arg(long = "for", value_name = "TIME", value_parser = crate::args::duration_arg)]
    pub(crate) seconds: Option<u64>,
    /// Also stop at the first event this jq filter picks out, printing it, whether it
    /// concerns you or not; what concerns you still prints and wakes you meanwhile.
    /// Such as, to wait for a landing: --until 'select(.message.event.kind ==
    /// "x-zirc/landing" and (.message.event.text | test("landed api-auth ")))'. A filter
    /// that outputs anything but false or null picks the event.
    #[arg(long = "until", value_name = "JQ", conflicts_with = "filter")]
    pub(crate) until_jq: Option<String>,
    /// Also stop at the first message saying just this, in any case, printing it: such as
    /// `zirc watch --said ping && zirc post pong`. --until does the same with a jq filter.
    #[arg(long, value_name = "TEXT", conflicts_with_all = ["filter", "until_jq"])]
    pub(crate) said: Option<String>,
    /// Wait quietly for the first thing that concerns you, print it and what comes with
    /// it, then stop: run it in the background and start it again after each wake.
    #[arg(long)]
    pub(crate) once: bool,
    /// Also print what another session says or what names it, such as how its landings
    /// went. Repeat for more.
    #[arg(long, value_name = "SESSION")]
    pub(crate) follow: Vec<String>,
    /// Also print everything people, agents and CI say in a channel this machine is in,
    /// such as #ops or #general, to keep an eye on it. Repeat for more.
    #[arg(long = "channel", value_name = "CHANNEL")]
    pub(crate) channels: Vec<String>,
    /// With --channel, print only what's said there at this level or above, as in most
    /// logging libraries: debug, info, warn or error. Such as --channel '#ops' --level warn,
    /// for what needs someone. What concerns you still prints whatever its level.
    #[arg(long, value_name = "LEVEL", value_parser = level, requires = "channels")]
    pub(crate) level: Option<Level>,
}

/// The level `word` names, in any case, as --level takes it.
fn level(word: &str) -> Result<Level, String> {
    match word.to_ascii_lowercase().as_str() {
        "debug" => Ok(Level::Debug),
        "info" => Ok(Level::Info),
        "warn" | "warning" => Ok(Level::Warn),
        "error" => Ok(Level::Error),
        _ => Err(format!("{word}: say debug, info, warn or error")),
    }
}

/// `zirc watch`.
pub(crate) async fn watch(
    socket: &Path,
    home: &Path,
    name: (Option<&str>, Option<&str>, Option<&str>),
    args: WatchArgs,
    out: &mut impl Write,
) -> Result<()> {
    let WatchArgs {
        all,
        json,
        session,
        filter,
        seconds,
        until_jq,
        said,
        once,
        follow,
        channels,
        level,
    } = args;
    name_session(socket, session.as_deref(), name).await?;
    let filter = filter
        .as_deref()
        .map(|jq| filter_of(jq, "--filter"))
        .transpose()?;
    let stop_at = match said {
        Some(said) => Some(Filter::said(&said)),
        None => until_jq
            .as_deref()
            .map(|jq| filter_of(jq, "--until"))
            .transpose()?,
    };
    let until = seconds.map(|seconds| tokio::time::Instant::now() + Duration::from_secs(seconds));
    // The repositories watched here, for their own hints: none if zircd can't say.
    let repos = match ask(socket, Request::RepoList).await {
        Ok(Response::Repos { repos }) => repos,
        _ => Vec::new(),
    };
    let hints = std::sync::Mutex::new(watch_hints(home, session.as_deref(), &repos));
    let how = How {
        all,
        json,
        filter: filter.as_ref(),
        stop_at: stop_at.as_ref(),
        follow: &follow,
        channels: &channels,
        level,
        once,
        hints: &hints,
    };
    watch_as(socket, &how, session.as_deref(), until, out).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use zirc_proto::Message;

    use crate::lines::tests::delivery;

    #[test]
    fn a_waking_line_shows_its_hint_once_and_json_carries_it() {
        let home = tempfile::tempdir().unwrap();
        let hints = std::sync::Mutex::new(watch_hints(home.path(), Some("cc-1"), &[]));
        let how = How {
            all: false,
            json: false,
            filter: None,
            stop_at: None,
            follow: &[],
            channels: &[],
            level: None,
            once: false,
            hints: &hints,
        };
        let failed = || {
            let mut it = delivery(Event::Custom {
                kind: zirc_proto::ci::LANDING_KIND.into(),
                data: serde_json::json!({"state": "failed", "build": "7", "branch": "fix-login"}),
                text: "cc-1: build 7 of fix-login failed: //web:e2e".into(),
            });
            it.why = Some(ipc::Why::Mention);
            Heard::Message(it)
        };
        let printed = |how: &How<'_>| {
            let mut out = Vec::new();
            print_heard(failed(), how, &mut out).unwrap();
            String::from_utf8(out).unwrap()
        };
        let first = printed(&how);
        assert!(
            first.ends_with(
                "\n  \u{21b3} hint (default): Fix it on `fix-login`, land again, and tell the user \
                 in one line. If your change doesn't touch what failed, run zirc state before \
                 you say so: someone may be on it.\n"
            ),
            "{first}"
        );
        // Once per kind, even across --once watches.
        let again = std::sync::Mutex::new(watch_hints(home.path(), Some("cc-1"), &[]));
        assert!(
            !printed(&How {
                hints: &again,
                ..how
            })
            .contains("hint")
        );
        // A fresh session's JSON carries it.
        let fresh = std::sync::Mutex::new(watch_hints(home.path(), Some("cc-2"), &[]));
        let json = printed(&How {
            json: true,
            hints: &fresh,
            ..how
        });
        let json: serde_json::Value = serde_json::from_str(json.trim()).unwrap();
        assert_eq!(json["hint"]["kind"], "landing.yours.failed");
        assert_eq!(json["hint"]["layer"], "default");
    }

    #[test]
    fn a_once_watch_waits_quietly_for_zircd_to_be_back() {
        // Ending wakes the session only to start the watch again.
        assert!(rewatch_wait(true) >= Duration::from_mins(30));
        assert_eq!(rewatch_wait(false), REWATCH_WAIT);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        let socket = home.path().join("zircd.sock");
        let gone = runtime
            .block_on(rewatch(
                &socket,
                None,
                Some("cc-1"),
                Duration::from_millis(300),
            ))
            .err()
            .unwrap()
            .to_string();
        assert!(gone.starts_with("zircd stopped"), "{gone}");
    }

    #[test]
    fn a_watch_whose_session_is_gone_ends() {
        assert!(is_orphan(1));
        assert!(!is_orphan(std::os::unix::process::parent_id()));
    }

    #[test]
    fn follows_another_sessions_posts_and_what_names_it() {
        let post = |session: &str, body: &str| {
            let mut delivery = delivery(Event::Text { body: body.into() });
            delivery.message.session = session.into();
            Heard::Message(delivery)
        };
        let followed = ["cc-2".to_owned()];
        assert!(follows(&post("cc-2", "taking: login"), &followed));
        assert!(follows(
            &post("ci", "cc-2: landed login as 1a2b3c (build 7)"),
            &followed
        ));
        assert!(!follows(&post("cc-3", "cc-22: landed"), &followed));
    }

    #[test]
    fn keeps_an_eye_on_a_channel_for_what_is_said_there() {
        let mut said = delivery(Event::Text {
            body: "deploying".into(),
        });
        said.channel = "#ops".into();
        let channels = ["#OPS".to_owned()];
        let Heard::Message(marked) = in_channel(Heard::Message(said.clone()), &channels, None)
        else {
            unreachable!()
        };
        assert_eq!(marked.why, Some(ipc::Why::Channel));
        let working = Delivery {
            message: Message::new(
                "watcher",
                Event::Working {
                    paths: vec!["a.rs".into()],
                    continued: false,
                    worktree: None,
                    branch: None,
                    part: None,
                    base: None,
                    committed: Vec::new(),
                },
            ),
            ..said.clone()
        };
        let Heard::Message(left) = in_channel(Heard::Message(working), &channels, None) else {
            unreachable!()
        };
        assert_eq!(left.why, None, "zircd's own events stay out");
        let Heard::Message(elsewhere) =
            in_channel(Heard::Message(said), &["#zirc".to_owned()], None)
        else {
            unreachable!()
        };
        assert_eq!(elsewhere.why, None);
    }

    #[test]
    fn a_channels_lines_below_the_level_asked_for_stay_out() {
        let at = |level: Option<Level>| {
            let mut said = delivery(Event::Text {
                body: "deploying".into(),
            });
            said.channel = "#ops".into();
            said.message.level = level;
            Heard::Message(said)
        };
        let channels = ["#ops".to_owned()];
        let why = |heard: Heard, least: Level| why(&in_channel(heard, &channels, Some(least)));
        // As in logging libraries: warn lets warn and error through.
        assert_eq!(
            why(at(Some(Level::Warn)), Level::Warn),
            Some(ipc::Why::Channel)
        );
        assert_eq!(
            why(at(Some(Level::Error)), Level::Warn),
            Some(ipc::Why::Channel)
        );
        assert_eq!(why(at(Some(Level::Info)), Level::Warn), None);
        // A post that says no level is info, as its kind says.
        assert_eq!(why(at(None), Level::Warn), None);
        assert_eq!(why(at(None), Level::Info), Some(ipc::Why::Channel));
        // What concerns the session already stays, whatever its level.
        let Heard::Message(mut mention) = at(Some(Level::Info)) else {
            unreachable!()
        };
        mention.why = Some(ipc::Why::Mention);
        assert_eq!(
            why(Heard::Message(mention), Level::Error),
            Some(ipc::Why::Mention)
        );
        assert_eq!(level("WARN"), Ok(Level::Warn));
        assert!(level("loud").is_err());
    }

    #[test]
    fn leaves_working_sets_out_unless_asked() {
        let working = delivery(Event::Working {
            paths: vec!["a.rs".into()],
            continued: false,
            worktree: None,
            branch: None,
            part: None,
            base: None,
            committed: Vec::new(),
        });
        assert!(is_noise(&Heard::Message(working)));
        let text = delivery(Event::Text { body: "hi".into() });
        assert!(!is_noise(&Heard::Message(text)));
    }

    #[test]
    fn refuses_a_filter_naming_a_kind_zirc_never_sends() {
        let refused = filter_of(r#"select(.message.event.kind == "x-zirc/push")"#, "--until")
            .err()
            .unwrap()
            .to_string();
        assert!(refused.starts_with("--until names x-zirc/push, a kind zirc never sends"));
        assert!(
            refused.contains(r#".message.event.type == "push""#),
            "{refused}"
        );
        assert!(filter_of(r#"select(.message.event.type == "push")"#, "--until").is_ok());
    }

    #[test]
    fn until_picks_out_the_event_its_filter_selects() {
        let landing = |text: &str| {
            Heard::Message(Delivery::new(
                "#monorepo/landings",
                "zirc-alerts",
                zirc_proto::Message::new(
                    "ci",
                    Event::Custom {
                        kind: "x-zirc/landing".into(),
                        data: serde_json::Value::Null,
                        text: text.into(),
                    },
                ),
            ))
        };
        let select = Filter::new(
            r#"select(.message.event.kind == "x-zirc/landing" and (.message.event.text | test("landed api-auth ")))"#,
        )
        .unwrap();
        let boolean = Filter::new(r#".message.event.text | contains("api-auth")"#).unwrap();
        let landed = landing("cc-1: landed api-auth as 81009e3 (build 1365)");
        let other = landing("cc-2: landed web-login as 1234567 (build 1366)");
        assert!(picks(Some(&select), &landed));
        assert!(!picks(Some(&select), &other));
        assert!(picks(Some(&boolean), &landed));
        assert!(!picks(Some(&boolean), &other), "false picks nothing");
        assert!(!picks(None, &landed));
        // A filter that fails on an event picks nothing, rather than ending the watch.
        let failing = Filter::new(".message.event.data.branch | test(\"x\")").unwrap();
        assert!(!picks(Some(&failing), &landed));
    }
}
