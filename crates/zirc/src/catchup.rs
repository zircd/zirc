//! `zirc catchup`: what people and agents said lately, from the server's history through
//! zircd's own connection, grouped by conversation: what names this machine's person and
//! direct messages first, then each channel. zircd's own events, such as working sets, are
//! left out: they're for zircd, not for reading.

use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use clap::Args;
use zirc_core::ipc::{Delivery, Request, Response};
use zirc_core::login::HistoryQuery;
use zirc_core::settings::Settings;
use zirc_proto::Event;

use crate::args::{OrgArg, moment_arg};
use crate::client::{ask, waits};
use crate::search;

/// How many messages of each conversation are read from the server's history: enough for
/// a few hours of a busy channel.
const READ: usize = 300;

/// How many of each conversation's messages are shown, the newest: the rest are counted.
const SHOWN: usize = 20;

/// What `zirc catchup` found.
#[derive(Debug)]
struct Catchup {
    /// How far back it looked, in seconds.
    since: u64,
    /// The moment it looked up to, when not now.
    until: Option<u64>,
    /// What names this machine's person, from any conversation.
    mentions: Vec<Said>,
    /// Each conversation with something said in it, direct messages first.
    conversations: Vec<Conversation>,
}

/// One channel or direct message's conversation.
#[derive(Debug)]
struct Conversation {
    /// The channel, or the nick a direct message is with.
    target: String,
    /// Whether it's a direct message rather than a channel.
    direct: bool,
    /// How many messages were said in it since.
    count: usize,
    /// The newest of them, oldest first.
    messages: Vec<Said>,
}

/// One message, as people read it.
#[derive(Debug, Clone)]
struct Said {
    /// When, as the server's `time` tag says.
    time: String,
    /// Who: a nick.
    from: String,
    /// The agent session that said it, if one did.
    session: String,
    /// Where, for a mention: the conversation's target.
    target: String,
    /// What, on one line.
    text: String,
    /// The server's id for it, which `zirc react` takes.
    id: Option<String>,
}

/// Runs `zirc catchup`: what was said from `from` to `until` (now when `None`), in the
/// conversations of one organisation, the one `org` names or else as
/// [`search::one_org`] picks: as this machine heard it, or with `team`, as the organisation's
/// policy lets its person read the server's store through zirc.dev.
pub(super) async fn catch_up(
    socket: &Path,
    home: &Path,
    (from, until): (u64, Option<u64>),
    org: Option<&str>,
    team: bool,
    json: bool,
    out: &mut impl Write,
) -> Result<()> {
    let status = match ask(socket, Request::Status).await {
        Ok(Response::Status(status)) => Some(status),
        // The team's history comes from zirc.dev, with or without zircd.
        _ if team => None,
        Ok(_) => return Err(crate::client::unexpected()),
        Err(err) => return Err(err),
    };
    let names = person_names(home, status.as_ref());
    let org = search::one_org(home, socket, org).await?;
    let conversations = match &status {
        Some(_) | None if team => {
            let team = search::team(&org).with_context(|| {
                format!(
                    "{} wasn't signed in through zirc.dev, so it has no team history to read: \
                     run zirc catchup without --team",
                    org.name()
                )
            })?;
            team_conversations(&team, (from, until)).await?
        }
        Some(status) => heard(socket, status, (from, until), &org.name()).await,
        None => Vec::new(),
    };
    let since = window(from, until.unwrap_or_else(now));
    let mut catchup = shaped(since, conversations, &names);
    catchup.until = until;
    if json {
        writeln!(out, "{}", to_json(&catchup))?;
    } else {
        print(&catchup, out)?;
    }
    Ok(())
}

/// How many conversations `--team` reads, those with the newest messages first.
const TEAM_CONVERSATIONS: usize = 15;

/// What was said from `from` to `until` in the conversations `team`'s policy lets this
/// machine's person read, from zirc.dev: the newest of each, with how many there were.
async fn team_conversations(
    team: &search::Team,
    (from, until): (u64, Option<u64>),
) -> Result<Vec<Conversation>> {
    let until = until.map(zirc_core::time::tag);
    let from = zirc_core::time::tag(from);
    let mut conversations = Vec::new();
    let overview = team
        .client
        .conversations(&team.account, &from)
        .await?
        .context("zirc.dev doesn't take this machine's account any more: run `zirc login`")?;
    for seen in overview.conversations.into_iter().take(TEAM_CONVERSATIONS) {
        let query = HistoryQuery {
            since: from.clone(),
            until: until.clone(),
            conversation: Some(seen.conversation.clone()),
            limit: SHOWN,
            ..HistoryQuery::default()
        };
        let page = search::read_page(team, &query).await?;
        // The overview counts everything since `from`: up to `until`, count what's there.
        let count = match (&until, &page.next) {
            (None, _) => seen.count,
            (Some(_), None) => page.messages.len(),
            (Some(_), Some(next)) => {
                window_count(team, &query, page.messages.len(), next.clone()).await?
            }
        };
        if count == 0 {
            continue;
        }
        let messages = page
            .messages
            .iter()
            .rev()
            .map(|message| Said {
                time: message.time.clone(),
                from: message.sender.nick.clone(),
                session: search::who(message)
                    .split_once('/')
                    .map(|(_, session)| session.to_owned())
                    .unwrap_or_default(),
                target: seen.conversation.clone(),
                text: crate::lines::one_line(&message.text),
                id: Some(message.msgid.clone()),
            })
            .collect();
        conversations.push(Conversation {
            target: seen.conversation,
            direct: seen.kind == "dm",
            count,
            messages,
        });
    }
    Ok(conversations)
}

/// How many messages `query` finds in all, the first page's `first` of them read and `next`
/// where the rest start: at most [`WINDOW_COUNT`], a page at a time.
async fn window_count(
    team: &search::Team,
    query: &HistoryQuery,
    first: usize,
    next: String,
) -> Result<usize> {
    let mut query = HistoryQuery {
        limit: WINDOW_PAGE,
        cursor: Some(next),
        ..query.clone()
    };
    let mut count = first;
    while count < WINDOW_COUNT {
        let page = search::read_page(team, &query).await?;
        count += page.messages.len();
        match page.next {
            Some(next) => query.cursor = Some(next),
            None => break,
        }
    }
    Ok(count.min(WINDOW_COUNT))
}

/// The most messages a conversation is counted up to in a range that ends before now.
const WINDOW_COUNT: usize = 1000;

/// How many messages a page of that counting asks for: the most the server gives.
const WINDOW_PAGE: usize = 200;

/// What was said from `from` to `until` in the conversations this machine is in in `org`, as
/// zircd's connection reads the server's history.
async fn heard(
    socket: &Path,
    status: &zirc_core::ipc::Status,
    (from, until): (u64, Option<u64>),
    org: &str,
) -> Vec<Conversation> {
    // The organisation's channels, named to zircd on a machine signed in to several.
    let orgs: Vec<(Option<String>, Vec<String>)> = if status.orgs.is_empty() {
        vec![(None, status.channels.clone())]
    } else {
        status
            .orgs
            .iter()
            .filter(|it| it.name == org)
            .map(|it| (Some(it.name.clone()), it.channels.clone()))
            .collect()
    };
    let mut conversations = Vec::new();
    for (org, channels) in orgs {
        // Direct messages, if zircd can say which conversations had any.
        let request = Request::Conversations {
            seconds: now().saturating_sub(from),
        };
        // The server's targets can list channels too: those are read as channels, below.
        let directs = match waits(socket, org.as_deref(), request).await {
            Ok(Response::Conversations { targets }) => targets
                .into_iter()
                .filter(|target| !target.starts_with('#'))
                .collect(),
            // The channels are still worth reading, but say what's missing.
            answer => {
                let why = refused(answer);
                eprintln!(
                    "zirc: catchup leaves out direct messages, as zircd couldn't list them: {why}"
                );
                Vec::new()
            }
        };
        let targets = directs
            .into_iter()
            .map(|target| (target, true))
            .chain(channels.into_iter().map(|channel| (channel, false)));
        for (target, direct) in targets {
            let request = Request::Read {
                target: target.clone(),
                limit: READ,
                since: Some(from),
                until,
            };
            // One zircd can't read is left out, and said: a conversation missing from a
            // catchup reads as nothing said in it (`zirc_cli_test`'s direct message, missing
            // once in about 400 runs with nothing to say why).
            let messages = match waits(socket, org.as_deref(), request).await {
                Ok(Response::Messages { messages }) => messages,
                answer => {
                    let why = refused(answer);
                    eprintln!(
                        "zirc: catchup leaves out {target}, as zircd couldn't read it: {why}"
                    );
                    continue;
                }
            };
            let said: Vec<Said> = messages
                .iter()
                .filter(|delivery| is_said(delivery))
                .filter(|delivery| {
                    sent_at(delivery)
                        .is_some_and(|at| at >= from && until.is_none_or(|until| at < until))
                })
                .map(|delivery| said(delivery, &target))
                .collect();
            if !said.is_empty() {
                conversations.push(Conversation {
                    target,
                    direct,
                    count: said.len(),
                    messages: said,
                });
            }
        }
    }
    conversations
}

/// Why zircd didn't give what catchup asked for, as `answer` says.
fn refused(answer: Result<Response>) -> String {
    match answer {
        Ok(Response::Error { message }) => message,
        Ok(other) => format!("{other:?}"),
        Err(err) => err.to_string(),
    }
}

/// What's found, shaped for reading: mentions of `names` picked out, direct messages
/// before channels, and each conversation cut to its newest [`SHOWN`].
fn shaped(since: u64, mut conversations: Vec<Conversation>, names: &[String]) -> Catchup {
    let mentions: Vec<Said> = conversations
        .iter()
        .filter(|conversation| !conversation.direct)
        .flat_map(|conversation| &conversation.messages)
        .filter(|said| {
            names
                .iter()
                .any(|name| zirc_core::names::mentions(&said.text, name))
        })
        .cloned()
        .collect();
    let mentions = mentions[mentions.len().saturating_sub(SHOWN)..].to_vec();
    conversations.sort_by_key(|conversation| !conversation.direct);
    for conversation in &mut conversations {
        let cut = conversation.messages.len().saturating_sub(SHOWN);
        conversation.messages.drain(..cut);
    }
    Catchup {
        since,
        until: None,
        mentions,
        conversations,
    }
}

/// What `zirc catchup --json` prints for `catchup`: fields left out when they say nothing.
fn to_json(catchup: &Catchup) -> serde_json::Value {
    let said = |said: &Said| {
        let mut json = serde_json::json!({"time": said.time, "from": said.from, "text": said.text});
        if let Some(id) = &said.id {
            json["id"] = id.clone().into();
        }
        if !said.session.is_empty() {
            json["session"] = said.session.clone().into();
        }
        json
    };
    let mentions: Vec<serde_json::Value> = catchup
        .mentions
        .iter()
        .map(|it| {
            let mut json = said(it);
            json["target"] = it.target.clone().into();
            json
        })
        .collect();
    let conversations: Vec<serde_json::Value> = catchup
        .conversations
        .iter()
        .map(|it| {
            serde_json::json!({
                "target": it.target,
                "direct": it.direct,
                "count": it.count,
                "messages": it.messages.iter().map(said).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::json!({
        "since": catchup.since,
        "until": catchup.until.map(zirc_core::time::tag),
        "mentions": mentions,
        "conversations": conversations,
    })
}

/// Prints `catchup` for people: a section for mentions, then one for each conversation.
fn print(catchup: &Catchup, out: &mut impl Write) -> Result<()> {
    let total: usize = catchup.conversations.iter().map(|it| it.count).sum();
    // "in the last 2h", or up to a moment: "in the 2h to 2026-11-05T08:00:00.000Z".
    let when = match catchup.until {
        None => format!("in the last {}", duration(catchup.since)),
        Some(until) => format!(
            "in the {} to {}",
            duration(catchup.since),
            zirc_core::time::tag(until)
        ),
    };
    if total == 0 {
        writeln!(out, "nothing said {when}")?;
        return Ok(());
    }
    writeln!(
        out,
        "{total} said in {} conversations {when}",
        catchup.conversations.len(),
    )?;
    if !catchup.mentions.is_empty() {
        writeln!(out, "\nnaming you ({}):", catchup.mentions.len())?;
        for said in &catchup.mentions {
            writeln!(out, "  {}", line(said, true))?;
        }
    }
    for conversation in &catchup.conversations {
        let kind = if conversation.direct { "dm with " } else { "" };
        writeln!(
            out,
            "\n{kind}{} ({}):",
            conversation.target, conversation.count
        )?;
        let hidden = conversation.count - conversation.messages.len();
        if hidden > 0 {
            writeln!(out, "  … {hidden} earlier")?;
        }
        for said in &conversation.messages {
            writeln!(out, "  {}", line(said, false))?;
        }
    }
    Ok(())
}

/// `said` as a line: when (hours and minutes), who and what, and where if `place`.
fn line(said: &Said, place: bool) -> String {
    let when = said.time.get(11..16).unwrap_or_default();
    let who = if said.session.is_empty() {
        said.from.clone()
    } else {
        format!("{}/{}", said.from, said.session)
    };
    let id = crate::lines::id_of(said.id.as_deref());
    if place {
        format!("{when} {} {who}: {}{id}", said.target, said.text)
    } else {
        format!("{when} {who}: {}{id}", said.text)
    }
}

/// Whether `delivery` is something a person or agent said, rather than one of zircd's
/// own events: posts, and the events teams and CI send, such as landings.
fn is_said(delivery: &Delivery) -> bool {
    matches!(
        delivery.message.event,
        Event::Text { .. }
            | Event::Custom { .. }
            | Event::Query { .. }
            | Event::Answer { .. }
            | Event::Note { .. }
    )
}

fn said(delivery: &Delivery, target: &str) -> Said {
    Said {
        time: delivery.time.clone().unwrap_or_default(),
        from: delivery.from.clone(),
        session: delivery.message.session.clone(),
        target: target.to_owned(),
        // What others send may hold newlines: on one line, it can't pass for another.
        text: crate::lines::text_of(delivery)
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect(),
        id: delivery.id.clone(),
    }
}

/// When `delivery` was sent, in seconds since the Unix epoch, by its `time` tag.
fn sent_at(delivery: &Delivery) -> Option<u64> {
    zirc_core::time::seconds_at(delivery.time.as_deref()?)
}

/// The names that mean this machine's person: its owner's handle in each organisation, and
/// the machine's nicks, which people use for whoever's at it.
fn person_names(home: &Path, status: Option<&zirc_core::ipc::Status>) -> Vec<String> {
    let settings = Settings::of(home);
    let owners = settings.iter().flat_map(|settings| {
        settings
            .owner
            .iter()
            .cloned()
            .chain(
                settings
                    .organisations()
                    .iter()
                    .filter_map(|org| org.owner.clone()),
            )
            .collect::<Vec<_>>()
    });
    let nicks = status.into_iter().flat_map(|status| {
        status
            .nick
            .iter()
            .cloned()
            .chain(status.orgs.iter().filter_map(|org| org.nick.clone()))
            .collect::<Vec<_>>()
    });
    let mut names: Vec<String> = owners.chain(nicks).collect();
    names.sort();
    names.dedup();
    names
}

/// "30m", "2h" or "1d 4h": `seconds` for people.
fn duration(seconds: u64) -> String {
    match seconds {
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 && s % 3600 == 0 => format!("{}h", s / 3600),
        s if s < 86_400 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        s => format!("{}d", s / 86_400),
    }
}

/// How long from `from` to `until`, in seconds, to the minute: `--since 1h`, read a second
/// before `until` was, is an hour, not "1h 0m".
fn window(from: u64, until: u64) -> u64 {
    until.saturating_sub(from).saturating_add(30) / 60 * 60
}

pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

// `zirc catchup`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct CatchupArgs {
    /// How far back: such as 30m, 2h or 1d, or a moment, such as 2026-11-05 or
    /// 2026-11-05T08:00:00Z.
    #[arg(long, default_value = "30m")]
    pub(crate) since: String,
    /// Up to when, as --since takes it, rather than now.
    #[arg(long)]
    pub(crate) until: Option<String>,
    /// The whole team's conversations, as the organisation lets you read them, from
    /// zirc.dev, rather than only what this machine heard.
    #[arg(long)]
    pub(crate) team: bool,
    /// Print JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
    #[command(flatten)]
    pub(crate) org: OrgArg,
}

/// `zirc catchup`.
pub(crate) async fn catchup(
    socket: &Path,
    home: &Path,
    args: CatchupArgs,
    out: &mut impl Write,
) -> Result<()> {
    let CatchupArgs {
        since,
        until,
        team,
        json,
        org,
    } = args;
    let since = moment_arg(&since, "--since")?;
    let until = until.map(|it| moment_arg(&it, "--until")).transpose()?;
    let org = org.name;
    // Boxed: reading every conversation makes a large future.
    Box::pin(catch_up(
        socket,
        home,
        (since, until),
        org.as_deref(),
        team,
        json,
        out,
    ))
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(from: &str, text: &str) -> Said {
        Said {
            time: "2026-11-02T12:04:05.000Z".into(),
            from: from.into(),
            session: String::new(),
            target: "#api".into(),
            text: text.into(),
            id: None,
        }
    }

    fn conversation(target: &str, direct: bool, messages: Vec<Said>) -> Conversation {
        Conversation {
            target: target.into(),
            direct,
            count: messages.len(),
            messages,
        }
    }

    #[test]
    fn a_window_is_counted_to_the_minute() {
        assert_eq!(window(1000, 1000 + 3601), 3600);
        assert_eq!(window(1000, 1000 + 3599), 3600);
        assert_eq!(duration(window(1000, 1000 + 3601)), "1h");
        assert_eq!(window(1000, 900), 0);
    }

    #[test]
    fn puts_mentions_and_direct_messages_first_and_cuts_long_conversations() {
        let busy: Vec<Said> = (0..25).map(|i| said("bob", &format!("line {i}"))).collect();
        let channel = conversation(
            "#api",
            false,
            [busy, vec![said("ada", "mattias: can you look at login?")]].concat(),
        );
        let dm = conversation("ada", true, vec![said("ada", "/agent rebase please")]);
        let catchup = shaped(1800, vec![channel, dm], &["mattias".to_owned()]);

        assert_eq!(catchup.mentions.len(), 1);
        assert_eq!(catchup.mentions[0].text, "mattias: can you look at login?");
        let targets: Vec<&str> = catchup
            .conversations
            .iter()
            .map(|it| it.target.as_str())
            .collect();
        assert_eq!(targets, ["ada", "#api"]);
        let api = &catchup.conversations[1];
        assert_eq!(api.count, 26);
        assert_eq!(api.messages.len(), SHOWN);
        assert_eq!(
            api.messages.last().unwrap().text,
            "mattias: can you look at login?"
        );

        let mut printed = Vec::new();
        print(&catchup, &mut printed).unwrap();
        let printed = String::from_utf8(printed).unwrap();
        assert!(
            printed.starts_with("27 said in 2 conversations in the last 30m\n"),
            "{printed}"
        );
        assert!(printed.contains("\nnaming you (1):\n  12:04 #api ada: mattias: can you"));
        assert!(printed.contains("\ndm with ada (1):\n"));
        assert!(printed.contains("\n#api (26):\n  … 6 earlier\n"));
    }

    #[test]
    fn leaves_out_zircds_own_events() {
        let delivery =
            |event| Delivery::new("#api", "alpha", zirc_proto::Message::new("cc-1", event));
        assert!(is_said(&delivery(Event::Text { body: "hi".into() })));
        assert!(!is_said(&delivery(Event::Working {
            paths: vec!["a.rs".into()],
            continued: false,
            worktree: None,
            branch: None,
            part: None,
            base: None,
            committed: Vec::new(),
        })));
    }
}
