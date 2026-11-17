//! Saying things: `zirc post`, `dm`, `intend`, `clashes`, `join`, `leave` and `trim`.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail, ensure};
use clap::Args;
use zirc_core::intent_scope;
use zirc_core::ipc::{Request, Response, Target};

use crate::LONG_LINE;
use crate::args::{Destination, OrgArg, app_named, duration_arg, session_or_user, words};
use crate::client::{ask, ask_in, here, org_here, waits};
use crate::repo::{in_repo, repo_here};
use crate::sessions::{OWNS_HISTORY, card_named, forget_gone, name_session};

#[derive(Debug, Args)]
pub(crate) struct TopicArgs {
    /// The topic, such as auth or releases.
    pub(crate) topic: String,
    /// The organisation's topic, #<topic>, rather than the repository's, #<repo>/<topic>.
    #[arg(long)]
    pub(crate) org: bool,
    /// The repository whose topic it is. Defaults to the current directory.
    #[arg(long)]
    pub(crate) repo: Option<PathBuf>,
    /// The session joining or leaving, whose `zirc watch` hears the topic's posts.
    /// Defaults to `$ZIRC_SESSION`; without one, the machine does, for watchers that aren't
    /// a session.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: Option<String>,
}

/// `zirc trim`: message `id` removed from `target`'s history, once the server says.
pub(crate) async fn trim_message(
    socket: &Path,
    org: Option<&str>,
    target: String,
    id: String,
    reason: Option<String>,
    out: &mut impl Write,
) -> Result<()> {
    let request = Request::Trim {
        target: target.clone(),
        id: id.clone(),
        reason,
    };
    let Response::Done = waits(socket, org, request).await? else {
        return Err(crate::client::unexpected());
    };
    writeln!(out, "trimmed {id} from {target}")?;
    Ok(())
}

/// Says `text` to this machine's owner alone, from `session`.
pub(crate) async fn send_dm(
    socket: &Path,
    org: Option<&str>,
    text: String,
    to: Option<String>,
    session: Option<String>,
    out: &mut impl Write,
) -> Result<()> {
    let request = Request::Dm {
        session: session_or_user(session),
        text,
        to,
    };
    print_channel(ask_in(socket, org, request).await?, "sent to", out)
}

/// Tells the repository's channel what the session is about to change: first, for a session
/// What `session` is told of its user's other sessions on this machine
/// ([`zirc_core::siblings::brief`]): nothing when zircd can't say.
async fn siblings(socket: &Path, session: &str) -> String {
    let Ok(Response::Status(status)) = ask(socket, Request::Status).await else {
        return String::new();
    };
    let (Some(machine), Ok(Response::Who { channels })) =
        (status.nick, ask(socket, Request::Who).await)
    else {
        return String::new();
    };
    zirc_core::siblings::brief(&channels, &machine, session)
}

/// on no task, its `taking:` post, so one command does both; then who else is on those paths.
pub(crate) async fn intend(
    socket: &Path,
    paths: Vec<PathBuf>,
    summary: String,
    seconds: u64,
    session: Option<String>,
    out: &mut impl Write,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let takes = session.is_some();
    let session = session_or_user(session);
    if ends_work(&summary) {
        let request = Request::Post {
            repo: cwd,
            session,
            text: summary,
            to: Target::Default,
            app: None,
            everyone: false,
            force: false,
            reply: None,
        };
        print_posted(ask(socket, request).await?, out)?;
        writeln!(out, "{INTENT_ENDED}")?;
        return Ok(());
    }
    // Before the intent, which then carries the task it starts.
    if takes {
        take_task(socket, &cwd, &session, &summary, out).await?;
    }
    let request = Request::Intend {
        paths: paths.iter().map(|path| cwd.join(path)).collect(),
        repo: cwd.clone(),
        session: session.clone(),
        summary,
        seconds,
    };
    print_channel(ask(socket, request).await?, "told", out)?;
    // Told as it is: a path broader than the change is worth saying, not refusing.
    write!(out, "{}", intent_scope::warnings(&cwd, &paths))?;
    // Told already: who else is there is worth saying, but not worth failing over.
    if let Ok(Some((clashed, text))) = clashes_found(socket, &session, &paths).await {
        write!(out, "{text}")?;
        if !clashed {
            writeln!(out, "{}", zirc_core::clashes::NOBODY)?;
        }
    }
    write!(
        out,
        "{}",
        crate::gotchas::under_here(socket, &session, &paths).await
    )?;
    write!(out, "{}", siblings(socket, &session).await)?;
    Ok(())
}

/// Posts `taking: <summary>` for `session` when it's on no task, as `zirc post` would, so
/// its intent needn't follow a post of its own: the agent evals (16 Nov 2026) counted two
/// posts a session, mostly `taking:` and `done:`. A zircd too old to say a session's task
/// takes nothing.
async fn take_task(
    socket: &Path,
    cwd: &Path,
    session: &str,
    summary: &str,
    out: &mut impl Write,
) -> Result<()> {
    let org = org_here(socket, OrgArg::default()).await?;
    let request = Request::Task {
        session: session.to_owned(),
        landing: false,
    };
    let Ok(Response::Task { task: None }) = waits(socket, org.as_deref(), request).await else {
        return Ok(());
    };
    let text = zirc_proto::taking(summary);
    let request = Request::Post {
        repo: cwd.to_owned(),
        session: session.to_owned(),
        text: text.clone(),
        to: Target::Default,
        app: None,
        everyone: false,
        force: false,
        reply: None,
    };
    let Response::Channel { channel, .. } = ask(socket, request).await? else {
        return Err(crate::client::unexpected());
    };
    writeln!(
        out,
        "posted {text} to {channel}: zircd says done: once your landing lands"
    )?;
    Ok(())
}

/// What `zirc intend` says when its text ends the work rather than starting it.
const INTENT_ENDED: &str = "a done: or dropped: line ends your intent, so it was posted, not \
    said as an intent; next time, zirc post it";

/// Whether an intent's `summary` ends the work, `done:` or `dropped:`, rather than saying
/// what's next. Sessions sent intents reading "done: …, committed" (the chat analysis, 11
/// Nov 2026), which told everyone on those paths that they were about to change them.
fn ends_work(summary: &str) -> bool {
    let lower = summary.trim_start().to_lowercase();
    lower.starts_with("done:") || lower.starts_with("dropped:")
}

/// Why `zirc post` refuses a post that's hard to read, and how to write it instead.
pub(crate) const POST_REFUSED: &str = "not posted: a line is over 200 characters, or chains clauses with \
    semicolons. Write a summary line, then one `- ` bullet a point, with - to give the lines \
    on stdin; or post it as it is with --force";

/// Whether `text` would read better as a summary and bullets: a line of it, outside code
/// fences, is over [`LONG_LINE`] characters, or chains three or more clauses with
/// semicolons. Agents' posts were a median 268 characters, a third of them one unbroken
/// line over 280 (the chat analysis, 5 Nov 2026), though the agent guide asks for bullets.
pub(crate) fn hard_to_read(text: &str) -> bool {
    let mut fenced = false;
    text.lines().any(|line| {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            return false;
        }
        !fenced && (line.chars().count() > LONG_LINE || line.matches("; ").count() >= 2)
    })
}

/// Why `zirc post` refuses a post saying what landed, and what to say instead.
pub(crate) const LANDING_REFUSED: &str = "not posted: CI already says what landed, and as what, in the \
    repository's landings channel, and `zirc wait-for` wakes whoever waits on it. Post \
    `done: <the task>` without the commit or build, or nothing; or post it as it is with --force";

/// Whether `text` reports a landing as CI does: it says something landed or was queued, and
/// names the commit or build. In #monorepo/agents 43% of lines did, each a copy of a line CI
/// had already said (the channel analysis, 6 Nov 2026). A build that failed isn't one: a post
/// about it is asking for help or saying whose it is.
pub(crate) fn reports_a_landing(text: &str) -> bool {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| c.is_whitespace() || "()[],;:`\"".contains(c))
        .filter(|word| !word.is_empty())
        .collect();
    let landed = words
        .iter()
        .any(|word| matches!(*word, "landed" | "queued" | "lands"));
    let build = words
        .windows(2)
        .any(|pair| pair[0] == "build" && pair[1].trim_end_matches('.').parse::<u32>().is_ok());
    let commit = words.iter().any(|word| {
        let word = word.trim_end_matches('.');
        (7..=40).contains(&word.len())
            && word.chars().all(|c| c.is_ascii_hexdigit())
            && word.chars().any(|c| c.is_ascii_digit())
            && word.chars().any(|c| c.is_ascii_alphabetic())
    });
    landed && (build || commit) && !lower.contains("failed")
}

/// Who besides `session` is changing, or about to change, `paths` (relative to where this
/// runs) in the watched repository here, as [`zirc_core::clashes::text`] says it, then who
/// changed them lately, to ask how they work ([`zirc_core::owners::ask_text`]). Nothing
/// outside a watched repository.
pub(crate) async fn clashes_here(
    socket: &Path,
    session: &str,
    paths: &[PathBuf],
) -> Result<String> {
    Ok(clashes_found(socket, session, paths)
        .await?
        .map(|(_, text)| text)
        .unwrap_or_default())
}

/// [`clashes_here`], and whether anyone besides `session` is on `paths`: `None` outside a
/// watched repository, or one whose channel zircd doesn't know yet.
async fn clashes_found(
    socket: &Path,
    session: &str,
    paths: &[PathBuf],
) -> Result<Option<(bool, String)>> {
    let Response::Repos { repos } = ask(socket, Request::RepoList).await? else {
        return Err(crate::client::unexpected());
    };
    let cwd = std::env::current_dir()?;
    let cwd = cwd.canonicalize().unwrap_or(cwd);
    let Some(repo) = repo_here(&repos, &cwd) else {
        return Ok(None);
    };
    let root = repo
        .path
        .canonicalize()
        .unwrap_or_else(|_| repo.path.clone());
    let paths: Vec<String> = paths
        .iter()
        .filter_map(|path| in_repo(&root, &cwd.join(path)))
        .collect();
    let Response::Who { channels } = ask_in(socket, repo.org.as_deref(), Request::Who).await?
    else {
        return Err(crate::client::unexpected());
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let Some(channel) = channels
        .iter()
        .find(|channel| channel.channel == repo.channel)
    else {
        return Ok(None);
    };
    let clashes = zirc_core::clashes::clashes(channel, session, &paths, now);
    let mut text = zirc_core::clashes::text(&clashes, session);
    // Who changed them lately, to ask: worth saying, not worth failing over.
    let read = Request::Read {
        target: channel.channel.clone(),
        limit: OWNS_HISTORY,
        since: None,
        until: None,
    };
    if let Ok(Response::Messages { messages }) = waits(socket, repo.org.as_deref(), read).await {
        let mut owners = zirc_core::owners::owners_of(channel, &messages, &paths, session, now);
        forget_gone(socket, &mut owners).await;
        text.push_str(&zirc_core::owners::ask_text(&owners));
    }
    Ok(Some((!clashes.is_empty(), text)))
}

/// Joins or leaves a topic.
pub(crate) async fn topic(
    socket: &Path,
    args: TopicArgs,
    join: bool,
    out: &mut impl Write,
) -> Result<()> {
    let TopicArgs {
        topic,
        org,
        repo,
        session,
    } = args;
    let repo = here(repo)?;
    let (request, done) = if join {
        let request = Request::Join {
            repo,
            topic,
            org,
            session,
        };
        (request, "joined")
    } else {
        let request = Request::Leave {
            repo,
            topic,
            org,
            session,
        };
        (request, "left")
    };
    print_channel(ask(socket, request).await?, done, out)
}

/// Where a post or a note went: "posted to #zirc/login", or, for one addressed to an agent
/// session, "noted for mattias-mini/cc-272fe8".
pub(crate) fn print_posted(response: Response, out: &mut impl Write) -> Result<()> {
    match response {
        Response::Noted { to } => {
            writeln!(out, "noted for {to}: a note, which only its watch prints")?;
            Ok(())
        }
        response => print_channel(response, "posted to", out),
    }
}

/// "posted to #zirc/login", from zircd's answer naming the channel.
pub(crate) fn print_channel(response: Response, done: &str, out: &mut impl Write) -> Result<()> {
    let Response::Channel { channel, link } = response else {
        return Err(crate::client::unexpected());
    };
    match link {
        // A post's own link, to hand a person: it opens the post in their app.
        Some(link) => writeln!(out, "{done} {channel}: {link}")?,
        None => writeln!(out, "{done} {channel}")?,
    }
    Ok(())
}

/// `zirc trim`'s help, saying what removing a message does as every app says it
/// ([`zirc_core::trim`]).
pub(crate) fn trim_help() -> String {
    format!(
        "Remove a message from the server's history, such as a secret posted by mistake.\n\n\
         Find its id with `zirc search`. {} Other clients that support removing messages \
         (draft/message-redaction) take it out too. {} So does whatever anyone copied. The \
         server allows it for messages this machine sent, and for any in a channel it \
         operates.",
        zirc_core::trim::reach(),
        zirc_core::trim::KEPT,
    )
}

// `zirc post`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct PostArgs {
    /// What to say, or - to read it from stdin.
    #[arg(required = true, num_args = 1..)]
    pub(crate) text: Vec<String>,
    /// Who's saying it, such as an agent session's id. Defaults to $ZIRC_SESSION, then
    /// $USER.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: Option<String>,
    /// For an integration, such as CI or a deploy bot: the short name apps show for it in
    /// place of a generic app badge, such as ci or deploy.
    #[arg(long, value_name = "NAME")]
    pub(crate) app: Option<String>,
    /// Post it even with a line over 200 characters, or three clauses chained with
    /// semicolons, which zirc post otherwise refuses as hard to read; or to the repository's
    /// channel (--to repo) from an agent session, when it isn't for a person.
    #[arg(long)]
    pub(crate) force: bool,
    /// Wake every session that hears it, as a broadcast: rarely, for what they all need
    /// now. Without it, @everyone written in the text is only text.
    #[arg(long)]
    pub(crate) everyone: bool,
    /// Say it in your room in a public channel where this machine is your terminal, as
    /// `zirc login` named it, such as acme/web. Needed only on a machine that's in an
    /// organisation of its own too.
    #[arg(long = "in", value_name = "ORG/CHANNEL", conflicts_with_all = ["to", "topic"])]
    pub(crate) guest: Option<String>,
    /// Reply to a message, by the id zirc watch shows ([id 01jbx7ab]): it's said in the
    /// channel that message was, as a reply apps show under it, whatever --to says.
    #[arg(long, value_name = "ID", conflicts_with_all = ["to", "topic", "everyone"])]
    pub(crate) reply_to: Option<String>,
    #[command(flatten)]
    pub(crate) to: Destination,
}

// `zirc dm`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct DmArgs {
    /// What to say, or - to read it from stdin.
    #[arg(required = true, num_args = 1..)]
    pub(crate) text: Vec<String>,
    /// Whom to: a person's nick or account. Defaults to this machine's owner.
    #[arg(long)]
    pub(crate) to: Option<String>,
    /// Who's saying it, such as an agent session's id. Defaults to $ZIRC_SESSION, then
    /// $USER.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: Option<String>,
    #[command(flatten)]
    pub(crate) org: OrgArg,
}

// `zirc emit`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct EmitArgs {
    /// Its kind, such as x-acme/deploy.
    pub(crate) kind: String,
    /// What it says, for people, or - to read it from stdin.
    #[arg(required = true, num_args = 1..)]
    pub(crate) text: Vec<String>,
    /// What it carries: any JSON, such as '{"env":"prod","version":"1.4.2"}'.
    #[arg(long, value_name = "JSON")]
    pub(crate) json: Option<String>,
    /// A card for apps to draw, as JSON: {"id", "title", "state", "fields", "links",
    /// "actions", ...}, as docs/cards.md says. It goes in the data as "card", checked
    /// first; a later card with the same id replaces it.
    #[arg(long, value_name = "JSON")]
    pub(crate) card: Option<String>,
    /// For an integration, such as CI or a deploy bot: the short name apps show for it in
    /// place of a generic app badge, such as ci or deploy.
    #[arg(long, value_name = "NAME")]
    pub(crate) app: Option<String>,
    /// Who's sending it, such as an agent session's id or a bot's name. Defaults to
    /// $ZIRC_SESSION, then $USER.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: Option<String>,
    #[command(flatten)]
    pub(crate) to: Destination,
}

// `zirc clashes`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct ClashesArgs {
    /// Files or directories, relative to where you are.
    #[arg(required = true, num_args = 1..)]
    pub(crate) paths: Vec<PathBuf>,
    /// Whose changes they are: yours, which aren't counted. Defaults to $ZIRC_SESSION,
    /// then $USER.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: Option<String>,
}

// `zirc thinking`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct ThinkingArgs {
    /// Whom to: a person's nick, or a channel.
    #[arg(long)]
    pub(crate) to: String,
    /// How long at most, such as 90s or 5m: 2m unless said.
    #[arg(long = "for", value_name = "TIME", default_value = "2m", value_parser = duration_arg)]
    pub(crate) seconds: u64,
    /// Who's thinking, such as an agent session's id. Defaults to $ZIRC_SESSION, then
    /// $USER.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: Option<String>,
    #[command(flatten)]
    pub(crate) org: OrgArg,
}

// `zirc trim`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct TrimArgs {
    /// The channel, or the nick of the private conversation, it was sent to.
    pub(crate) target: String,
    /// The message's id, from `zirc search` or `zirc watch --json`.
    #[arg(required_unless_present = "last")]
    pub(crate) id: Option<String>,
    /// Remove the last thing you said there, without looking its id up: the newest message
    /// this machine sent as your session.
    #[arg(long, conflicts_with = "id")]
    pub(crate) last: bool,
    /// With --last, whose message: an agent session's id. Defaults to $ZIRC_SESSION, then
    /// $USER.
    #[arg(long, env = "ZIRC_SESSION", requires = "last")]
    pub(crate) session: Option<String>,
    /// Why, which the server passes on to those it tells.
    #[arg(long)]
    pub(crate) reason: Option<String>,
    #[command(flatten)]
    pub(crate) org: OrgArg,
}

/// `zirc post`.
pub(crate) async fn post(
    socket: &Path,
    name: (Option<&str>, Option<&str>, Option<&str>),
    args: PostArgs,
    out: &mut impl Write,
) -> Result<()> {
    let PostArgs {
        text,
        session,
        to,
        app,
        force,
        everyone,
        guest,
        reply_to,
    } = args;
    let (repo, to) = to.target();
    let session = session_or_user(session);
    name_session(socket, Some(&session), name).await?;
    let from_stdin = matches!(text.as_slice(), [dash] if dash == "-");
    let text = crate::sealed::with_stdin_part(words(&text)?, from_stdin, || {
        let mut read = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut read).map(|_| read)
    })?;
    if hard_to_read(&text) && !force {
        bail!("{POST_REFUSED}");
    }
    if reports_a_landing(&text) && !force {
        bail!("{LANDING_REFUSED}");
    }
    let note = (!everyone && text.to_lowercase().contains("@everyone")).then_some(
        "note: it didn't broadcast: @everyone in a post's text is only text. Post with \
         --everyone to wake every session, and only when they all need it",
    );
    let org = guest.as_deref().map(crate::guest::org_of);
    let answer = if zirc_core::sealed::split(&text).parts.is_empty() {
        let request = Request::Post {
            repo: here(repo)?,
            session,
            text,
            to,
            app: app_named(app)?,
            everyone,
            force,
            reply: reply_to,
        };
        ask_in(socket, org.as_deref(), request).await?
    } else {
        ensure!(
            app.is_none() && !everyone,
            "a post with sealed parts goes without --app or --everyone"
        );
        let request = Request::SealedPost {
            repo: here(repo)?,
            session,
            text,
            to,
            force,
            reply: reply_to,
        };
        // A zircd from before sealing refuses it, rather than posting the parts unsealed.
        waits(socket, org.as_deref(), request).await?
    };
    print_posted(answer, out)?;
    if let Some(note) = note {
        writeln!(out, "{note}")?;
    }
    Ok(())
}

/// `zirc dm`.
pub(crate) async fn dm(socket: &Path, args: DmArgs, out: &mut impl Write) -> Result<()> {
    let DmArgs {
        text,
        to,
        session,
        org,
    } = args;
    let org = org_here(socket, org).await?;
    send_dm(socket, org.as_deref(), words(&text)?, to, session, out).await
}

/// `zirc emit`.
pub(crate) async fn emit(socket: &Path, args: EmitArgs, out: &mut impl Write) -> Result<()> {
    let EmitArgs {
        kind,
        text,
        json,
        card,
        session,
        to,
        app,
    } = args;
    let mut data = match json {
        Some(json) => serde_json::from_str(&json).context("--json isn't JSON")?,
        None => serde_json::Value::Null,
    };
    if let Some(card) = card {
        data = card_named(&card)?.into_data(data);
    }
    let (repo, to) = to.target();
    let request = Request::Emit {
        repo: here(repo)?,
        session: session_or_user(session),
        kind,
        data,
        text: words(&text)?,
        to,
        app: app_named(app)?,
    };
    print_channel(ask(socket, request).await?, "sent to", out)
}

/// `zirc clashes`.
pub(crate) async fn clashes(socket: &Path, args: ClashesArgs, out: &mut impl Write) -> Result<()> {
    let ClashesArgs { paths, session } = args;
    let text = clashes_here(socket, &session_or_user(session), &paths).await?;
    write!(out, "{text}")?;
    Ok(())
}

/// `zirc thinking`.
pub(crate) async fn thinking(
    socket: &Path,
    args: ThinkingArgs,
    out: &mut impl Write,
) -> Result<()> {
    let ThinkingArgs {
        to,
        seconds,
        session,
        org,
    } = args;
    let request = Request::Typing {
        repo: std::env::current_dir()?,
        session: session_or_user(session),
        to,
        seconds,
    };
    print_channel(
        ask_in(socket, org.name.as_deref(), request).await?,
        "thinking in",
        out,
    )
}

/// `zirc trim`.
pub(crate) async fn trim(socket: &Path, args: TrimArgs, out: &mut impl Write) -> Result<()> {
    let TrimArgs {
        target,
        id,
        last,
        session,
        reason,
        org,
    } = args;
    let org = org_here(socket, org).await?;
    let id = match id {
        Some(id) if !last => id,
        _ => last_said(socket, org.as_deref(), &target, &session_or_user(session)).await?,
    };
    trim_message(socket, org.as_deref(), target, id, reason, out).await
}

/// How many of a conversation's newest messages `zirc trim --last` looks through.
const LAST_AMONG: usize = 50;

/// The id of the newest message in `target` that this machine sent as `session`, for
/// `zirc trim --last`: a mistake is removed by whoever made it, without hunting for its id.
async fn last_said(
    socket: &Path,
    org: Option<&str>,
    target: &str,
    session: &str,
) -> Result<String> {
    let Response::Status(status) = ask_in(socket, org, Request::Status).await? else {
        return Err(crate::client::unexpected());
    };
    let me = status.nick.unwrap_or_default();
    let read = Request::Read {
        target: target.to_owned(),
        limit: LAST_AMONG,
        since: None,
        until: None,
    };
    let Response::Messages { messages } = waits(socket, org, read).await? else {
        return Err(crate::client::unexpected());
    };
    newest_of(&messages, &me, session).with_context(|| {
        format!(
            "{session} said nothing among the newest {LAST_AMONG} messages in {target}: give \
             the id, which `zirc search --in {target}` shows"
        )
    })
}

/// The id of the newest of `messages`, oldest first, that the machine `me` sent as `session`.
fn newest_of(messages: &[zirc_core::ipc::Delivery], me: &str, session: &str) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|said| said.from.eq_ignore_ascii_case(me) && said.message.session == session)
        .and_then(|said| said.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use zirc_core::paths;

    use crate::{Cli, run};

    /// Every command's summary fits on one line of `zirc --help`; details go in a paragraph
    /// of their own, which `zirc <command> --help` shows.
    #[test]
    fn a_post_hard_to_read_is_refused() {
        let long = "word ".repeat(50);
        assert!(hard_to_read(&long), "a line over 200 characters");
        assert!(hard_to_read(
            "landed the fix; the tests pass; the release waits on the user"
        ));
        assert!(!hard_to_read("landed the fix; tests pass"));
        let bullets = format!("landed the fix\n- {}\n- tests pass", "short ".repeat(20));
        assert!(!hard_to_read(&bullets));
        // What's in a code block is as it is.
        let fenced = format!("the failure:\n```\n{long}\n```");
        assert!(!hard_to_read(&fenced));
    }

    #[test]
    fn a_post_saying_what_landed_is_ci_s_to_say() {
        // Lines from #monorepo/agents, 5 and 6 Nov 2026, that repeated CI.
        for text in [
            "done: compact-rows landed as f26be792 (build 1156)",
            "cc-272fe8: desktop agent row in the UI font landed as c142681 (build 1212)",
            "cc-272fe8: item 9 queued as build 1307 (`1680277`)",
            "done: both changing on web, landed as 41c939f (build 1492)",
        ] {
            assert!(reports_a_landing(text), "{text}");
        }
        for text in [
            "done: zircd tests split into topic files",
            "taking: urgent fix, the tenant controller crash-loops",
            "build 1009's //web:e2e is mine; the fix lands next",
            "cc-b4ec58: build 1514 failed, landed commits since 2cceca7 look fine",
            "done: step 4 committed on plan-page, not landed",
        ] {
            assert!(!reports_a_landing(text), "{text}");
        }
    }

    #[test]
    fn an_intent_saying_done_or_dropped_ends_the_work() {
        for text in [
            "done: profile cards, committed",
            "  Dropped: the old sidebar",
        ] {
            assert!(ends_work(text), "{text}");
        }
        for text in [
            "profile cards: done is a word here",
            "split the done: handler",
        ] {
            assert!(!ends_work(text), "{text}");
        }
    }

    #[tokio::test]
    async fn refuses_a_post_hard_to_read_unless_forced() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(paths::config(home.path()), "[server]\nhost = \"x\"\n").unwrap();
        let long = "word ".repeat(50);
        let run_in = |args: Vec<&str>| {
            let cli = Cli::try_parse_from(args).unwrap();
            let cli = Cli {
                home: Some(home.path().to_owned()),
                ..cli
            };
            async move { run(cli, &mut Vec::new()).await }
        };
        let err = run_in(vec!["zirc", "post", "--session", "cc-1", &long])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.starts_with("not posted:"), "{err}");
        assert!(err.contains("--force"), "{err}");
        // Forced, it goes on to post, and meets the zircd that isn't running.
        let err = run_in(vec!["zirc", "post", "--session", "cc-1", "--force", &long])
            .await
            .unwrap_err()
            .to_string();
        assert!(!err.starts_with("not posted:"), "{err}");
    }

    #[test]
    fn trim_last_takes_the_sessions_own_newest_message_and_no_one_elses() {
        let said = |from: &str, session: &str, id: &str| zirc_core::ipc::Delivery {
            id: Some(id.to_owned()),
            ..zirc_core::ipc::Delivery::new(
                "#api",
                from,
                zirc_proto::Message::new(session, zirc_proto::Event::Text { body: "x".into() }),
            )
        };
        let messages = [
            said("mbp", "cc-1", "a1"),
            said("mbp", "cc-1", "a2"),
            said("mbp", "cc-2", "b1"),
            said("mini", "cc-1", "c1"),
        ];
        assert_eq!(newest_of(&messages, "MBP", "cc-1").as_deref(), Some("a2"));
        assert_eq!(newest_of(&messages, "mbp", "cc-2").as_deref(), Some("b1"));
        assert_eq!(newest_of(&messages, "mbp", "cc-9"), None);
        // Asked for by flag or by id, never both, and --last needs no id.
        Cli::try_parse_from(["zirc", "trim", "#api", "--last", "--session", "cc-1"]).unwrap();
        Cli::try_parse_from(["zirc", "trim", "#api", "01jbx7ab"]).unwrap();
        assert!(Cli::try_parse_from(["zirc", "trim", "#api"]).is_err());
        assert!(Cli::try_parse_from(["zirc", "trim", "#api", "01jbx7ab", "--last"]).is_err());
    }
}
