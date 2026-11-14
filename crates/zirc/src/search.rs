//! `zirc search`, zirc's one way to read history, and `zirc catchup --team`'s reading: the
//! organisation's history as its policy lets this machine's person read it, from the
//! server's own store through zirc.dev, beyond what this machine's own connection has heard.
//! A machine not signed in through zirc.dev, such as one on a server a team runs itself,
//! reads a channel's history from its own server through zircd instead.

use std::io::Write;
use std::path::Path;
use zirc_proto::PROGRAM;

use anyhow::{Context as _, Result, bail, ensure};
use clap::Args;
use zirc_core::ipc::{Delivery, Request, Response};
use zirc_core::login::{Client, HistoryMessage, HistoryPage, HistoryQuery};
use zirc_core::settings::{Organisation, PUBLIC_ORG, Settings};
use zirc_proto::Event;
use zirc_proto::login::Account;
use zirc_proto::login::HistorySender;

/// The most messages a search reads, a page at a time.
const MOST: usize = 1000;

/// How many messages a page asks for: the most the server gives.
const PAGE: usize = 200;

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Text the messages must have.
    text: Vec<String>,
    /// Only in this channel, or direct messages with this person's account.
    #[arg(long = "in", value_name = "CONVERSATION")]
    conversation: Option<String>,
    /// Only what this account or nick said.
    #[arg(long, value_name = "WHO")]
    from: Option<String>,
    /// How far back: such as 30m, 2h, 1d or 14d, or a moment, such as 2026-11-05 or
    /// 2026-11-05T08:00:00Z.
    #[arg(long, default_value = "1d")]
    since: String,
    /// Up to when, as --since takes it, rather than now: with --since, a range in the past,
    /// such as --since 2026-11-05T09:30:00Z --until 2026-11-05T10:30:00Z.
    #[arg(long)]
    until: Option<String>,
    /// The most messages to show, the newest.
    #[arg(long, default_value_t = 100)]
    limit: usize,
    /// zircd's events too, such as commits, pushes and working sets.
    #[arg(long)]
    events: bool,
    /// One JSON object a line, oldest first, for scripts and agents.
    #[arg(long)]
    json: bool,
    /// Only this organisation, on a machine signed in to several.
    #[arg(long, value_name = "NAME", hide = true)]
    org: Option<String>,
}

/// An organisation this machine reads history in: its name, the machine's account and the
/// web app it signed in at.
pub(super) struct Team {
    pub name: String,
    pub account: Account,
    pub client: Client,
}

/// The organisation a command that reads one acts on ([`zirc_core::which_org`]): the one
/// `named` with --org, else the one the repository it's run in belongs to, else the machine's
/// only own one. With several and nothing saying which, it fails asking for --org.
pub(super) async fn one_org(
    home: &Path,
    socket: &Path,
    named: Option<&str>,
) -> Result<Organisation> {
    let settings = Settings::of(home).context("this machine isn't set up: run `zirc login`")?;
    let repo = match named {
        Some(_) => None,
        None => crate::client::org_here(socket, crate::args::OrgArg::default())
            .await
            .ok()
            .flatten(),
    };
    Ok(settings.pick(named, repo.as_deref())?)
}

/// `org`'s history through zirc.dev, when this machine signed in to it there: not the public
/// server, a guest's terminal or a server a team runs itself, which zircd reads instead.
pub(super) fn team(org: &Organisation) -> Option<Team> {
    if org.name() == PUBLIC_ORG || org.guest.is_some() {
        return None;
    }
    Some(Team {
        name: org.name(),
        account: org.account.clone()?,
        client: Client::new(org.web.as_deref()?).ok()?,
    })
}

/// Runs `zirc search`.
pub async fn search(
    home: &Path,
    socket: &Path,
    args: SearchArgs,
    out: &mut impl Write,
) -> Result<()> {
    let args = read_tokens(args)?;
    let since_at = crate::args::moment_arg(&args.since, "--since")?;
    let until_at = args
        .until
        .as_deref()
        .map(|until| crate::args::moment_arg(until, "--until"))
        .transpose()?;
    if let Some(until_at) = until_at {
        ensure!(
            since_at < until_at,
            "--since is after --until: give the earlier first"
        );
    }
    let since = zirc_core::time::tag(since_at);
    let until = until_at.map(zirc_core::time::tag);
    let org = one_org(home, socket, args.org.as_deref()).await?;
    let mut args = args;
    args.org = Some(org.name());
    // A guest's terminal reads its own room, which it needn't name.
    if let (None, Some(guest)) = (&args.conversation, &org.guest) {
        args.conversation = Some(guest.channel.clone());
    }
    let mut found = match team(&org) {
        Some(team) => from_team(&team, &args, &since, until.as_deref()).await?,
        // Not signed in through zirc.dev: what this machine's own server keeps.
        None => from_zircd(socket, &args, since_at, until_at).await?,
    };
    // The newest `limit`, oldest first.
    found.sort_by(|a, b| b.time.cmp(&a.time));
    found.truncate(args.limit.min(MOST));
    if found.is_empty() && !args.json {
        let place = args
            .conversation
            .as_deref()
            .map(|it| format!(" in {it}"))
            .unwrap_or_default();
        match &until {
            Some(until) => writeln!(out, "nothing found{place} from {since} to {until}")?,
            None => writeln!(out, "nothing found{place} since {since}")?,
        }
    }
    for message in found.into_iter().rev() {
        if args.json {
            writeln!(out, "{}", serde_json::to_value(&message)?)?;
        } else {
            writeln!(out, "{}", line(&message))?;
        }
    }
    Ok(())
}

/// `args` with what its words say as a search (`in:#api from:leo after:2026-11-01 timeout`),
/// read as every app reads one ([`zirc_proto::search::Query`]); a flag given says the same and
/// wins. `in:*` is everywhere, as without `--in`.
fn read_tokens(mut args: SearchArgs) -> Result<SearchArgs> {
    use zirc_proto::search::{Query, Scope};
    if args.text.is_empty() {
        return Ok(args);
    }
    let query = Query::parse(&args.text.join(" "), None).map_err(|e| anyhow::anyhow!("{e}"))?;
    match query.scope {
        Scope::Conversation(name) => {
            args.conversation.get_or_insert(name);
        }
        Scope::Agent { .. } | Scope::AllServers => {
            bail!(
                "zirc search looks in one conversation (in:#channel, in:@nick) or everywhere (in:*)"
            )
        }
        Scope::Server => {}
    }
    if args.from.is_none() {
        args.from = query.from;
    }
    if query.kinds.iter().any(|kind| kind != "message") {
        args.events = true;
    }
    if let Some(after) = query.after {
        args.since = after;
    }
    if let (None, Some(before)) = (&args.until, query.before) {
        args.until = Some(before);
    }
    let text = query.text.trim().trim_matches('"');
    args.text = if text.is_empty() {
        Vec::new()
    } else {
        vec![text.to_owned()]
    };
    Ok(args)
}

/// What `args` finds in `team`'s history from `since` to `until`, through zirc.dev.
async fn from_team(
    team: &Team,
    args: &SearchArgs,
    since: &str,
    until: Option<&str>,
) -> Result<Vec<HistoryMessage>> {
    let mut found: Vec<HistoryMessage> = Vec::new();
    let mut query = HistoryQuery {
        since: since.to_owned(),
        until: until.map(str::to_owned),
        conversation: args.conversation.clone(),
        sender: args.from.clone(),
        text: (!args.text.is_empty()).then(|| args.text.join(" ")),
        limit: PAGE.min(args.limit.min(MOST)),
        events: args.events,
        ..HistoryQuery::default()
    };
    let mut read = 0;
    loop {
        let page = read_page(team, &query).await?;
        if read == 0 && page.policy != "participant" && !args.json {
            eprintln!(
                "{PROGRAM}: {} lets its people read {}",
                team.name,
                policy(&page.policy)
            );
        }
        read += page.messages.len();
        found.extend(page.messages);
        match page.next {
            Some(next) if read < args.limit.min(MOST) => query.cursor = Some(next),
            _ => break,
        }
    }
    Ok(found)
}

/// What `args` finds in one conversation from `since` to `until`, from this machine's own
/// server through zircd: for a machine not signed in through zirc.dev, which reads only the
/// channels and private conversations its connection is in.
async fn from_zircd(
    socket: &Path,
    args: &SearchArgs,
    since: u64,
    until: Option<u64>,
) -> Result<Vec<HistoryMessage>> {
    let Some(target) = args.conversation.clone() else {
        bail!(
            "this machine reads history from its own server, a conversation at a time: say \
             which with --in, such as --in '#api'"
        );
    };
    let request = Request::Read {
        target,
        limit: MOST,
        since: Some(since),
        until,
    };
    let Response::Messages { messages } =
        crate::client::waits(socket, args.org.as_deref(), request).await?
    else {
        return Err(crate::client::unexpected());
    };
    Ok(messages
        .iter()
        .filter(|delivery| wanted(delivery, args))
        .map(history_message)
        .collect())
}

/// Whether `delivery` is one `args` asks for: never a working set; zircd's other events only
/// with --events; with words, those that say them all; with --from, those from that nick,
/// account or session.
fn wanted(delivery: &Delivery, args: &SearchArgs) -> bool {
    let event = &delivery.message.event;
    if matches!(event, Event::Working { .. }) || (!args.events && event.is_machine()) {
        return false;
    }
    let text = event.human_text().to_lowercase();
    let says = args
        .text
        .iter()
        .all(|word| text.contains(&word.to_lowercase()));
    let from = args.from.as_ref().is_none_or(|who| {
        [
            Some(delivery.from.as_str()),
            delivery.account.as_deref(),
            Some(delivery.message.session.as_str()),
        ]
        .into_iter()
        .flatten()
        .any(|it| it.eq_ignore_ascii_case(who))
    });
    says && from
}

/// `delivery` as zirc.dev's history says a message, so both ways print alike. A machine's
/// account is its owner's handle and its name (`ada-mbp`), which a person's never has.
fn history_message(delivery: &Delivery) -> HistoryMessage {
    let agent = delivery
        .account
        .as_deref()
        .is_some_and(|account| account.contains('-'));
    HistoryMessage {
        msgid: delivery.id.clone().unwrap_or_default(),
        time: delivery.time.clone().unwrap_or_default(),
        conversation: delivery.channel.clone(),
        sender: HistorySender {
            account: delivery.account.clone(),
            nick: delivery.from.clone(),
            kind: if agent { "agent" } else { "person" }.to_owned(),
        },
        text: crate::lines::text_of(delivery),
        zirc: serde_json::to_value(&delivery.message).ok(),
        support: None,
    }
}

/// A page of `team`'s history for `query`.
pub(super) async fn read_page(team: &Team, query: &HistoryQuery) -> Result<HistoryPage> {
    team.client
        .history(&team.account, query)
        .await?
        .context("zirc.dev doesn't take this machine's account any more: run `zirc login`")
}

/// What a policy lets people read, in words.
fn policy(policy: &str) -> &str {
    match policy {
        "channels" => "every channel",
        "everything" => "every channel and everyone's direct messages",
        _ => "the channels they're in and their own direct messages",
    }
}

/// `message` as a line: when, where and who said what, on one line.
fn line(message: &HistoryMessage) -> String {
    let when = message
        .time
        .get(..16)
        .unwrap_or(&message.time)
        .replace('T', " ");
    format!(
        "{when} {} {}: {}{}",
        message.conversation,
        who(message),
        crate::lines::one_line(&message.text),
        crate::lines::id_of(Some(&message.msgid))
    )
}

/// Who said `message`: their nick, and the agent session that said it, if one did.
pub(super) fn who(message: &HistoryMessage) -> String {
    let session = message
        .zirc
        .as_ref()
        .and_then(|zirc| zirc.get("session")?.as_str())
        .filter(|session| !session.is_empty());
    match session {
        Some(session) => format!("{}/{session}", message.sender.nick),
        None => message.sender.nick.clone(),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    /// `zirc search <words>` as clap reads it, then its tokens.
    fn read(words: &[&str]) -> anyhow::Result<super::SearchArgs> {
        #[derive(clap::Parser)]
        struct Wrap {
            #[command(flatten)]
            args: super::SearchArgs,
        }
        super::read_tokens(Wrap::try_parse_from([&["search"], words].concat())?.args)
    }

    #[test]
    fn words_read_as_a_search_and_flags_win() {
        let args = read(&[
            "in:#API",
            "from:@leo",
            "after:2026-11-01",
            "type:event",
            "\"gateway down\"",
        ])
        .unwrap();
        assert_eq!(args.conversation.as_deref(), Some("#api"));
        assert_eq!(args.from.as_deref(), Some("leo"));
        assert_eq!(args.since, "2026-11-01");
        assert!(args.events);
        assert_eq!(args.text, ["gateway down"]);
        let flags = read(&["--in", "#web", "--from", "ada", "in:#api", "from:leo", "x"]).unwrap();
        assert_eq!(
            (flags.conversation.as_deref(), flags.from.as_deref()),
            (Some("#web"), Some("ada"))
        );
        let everywhere = read(&["in:*", "timeout"]).unwrap();
        assert_eq!(
            (everywhere.conversation, everywhere.text),
            (None, vec!["timeout".to_owned()])
        );
        assert!(read(&["after:yesterday"]).is_err());
        assert!(read(&["in:mbp/cc-1", "x"]).is_err());
    }

    use crate::{Cli, run};
    use zirc_core::paths;

    /// Runs `zirc <args>` as a machine of ada's signed in through the web app `web`.
    async fn zirc(web: &str, args: &[&str]) -> Result<String, String> {
        let home = tempfile::tempdir().unwrap();
        let config = format!(
            "web = \"{web}\"\nowner = \"ada\"\n[server]\nhost = \"acme.irc.zirc.dev\"\n[account]\nname = \"ada-mbp\"\npassword = \"pw\"\n"
        );
        std::fs::write(paths::config(home.path()), config).unwrap();
        let cli = Cli::try_parse_from([&["zirc"], args].concat()).unwrap();
        let cli = Cli {
            home: Some(home.path().to_owned()),
            ..cli
        };
        let mut out = Vec::new();
        run(cli, &mut out)
            .await
            .map(|()| String::from_utf8(out).unwrap())
            .map_err(|err| err.to_string())
    }

    fn message(time: &str, conversation: &str, nick: &str, text: &str) -> serde_json::Value {
        serde_json::json!({
            "msgid": format!("{}xyzabc", time.get(11..16).unwrap_or_default().replace(':', "")),
            "time": time,
            "conversation": conversation,
            "sender": {"account": nick, "nick": nick, "kind": "person"},
            "type": "privmsg",
            "text": text,
            "zirc": null,
        })
    }

    /// zirc.dev's answer to the ask for a token for the server: read through zirc.dev, as
    /// for a server that doesn't answer content itself (zirc-core's `Client::history`).
    fn through_zirc_dev() -> (u16, String) {
        (
            404,
            r#"{"error":"This server's history is read through zirc.dev."}"#.into(),
        )
    }

    fn page(policy: &str, messages: &[serde_json::Value], next: Option<&str>) -> String {
        serde_json::json!({"policy": policy, "messages": messages, "next": next}).to_string()
    }

    #[tokio::test]
    async fn searches_every_page_and_prints_the_oldest_first() {
        let (web, served) = zirc_test_ergo::web::serve(vec![
            through_zirc_dev(),
            (
                200,
                page(
                    "channels",
                    &[message(
                        "2026-11-02T12:05:00Z",
                        "#ops",
                        "bob",
                        "rolled back",
                    )],
                    Some("c1"),
                ),
            ),
            (
                200,
                page(
                    "channels",
                    &[message(
                        "2026-11-02T12:01:00Z",
                        "#ops",
                        "ada",
                        "login is down\nagain",
                    )],
                    None,
                ),
            ),
        ]);
        let out = zirc(&web, &["search", "login", "--in", "#ops", "--since", "2h"])
            .await
            .unwrap();
        assert_eq!(
            out,
            "2026-11-02 12:01 #ops ada: login is down again [id 1201xyza]\n\
             2026-11-02 12:05 #ops bob: rolled back [id 1205xyza]\n"
        );
        let requests = served.join().unwrap();
        let first = |i: usize| requests[i].lines().next().unwrap().to_owned();
        assert!(
            first(0).starts_with("POST /api/cli/content-token "),
            "{}",
            first(0)
        );
        assert!(
            first(1).starts_with("GET /api/cli/history?since="),
            "{}",
            first(1)
        );
        assert!(first(1).contains("&conversation=%23ops&q=login&limit=100 "));
        assert!(first(2).contains("&cursor=c1"), "{}", first(2));
        assert!(requests[1].to_lowercase().contains("authorization: basic"));
    }

    #[tokio::test]
    async fn on_a_machine_in_two_organisations_it_reads_the_one_org_names() {
        let (acme, _a) = zirc_test_ergo::web::serve(vec![
            through_zirc_dev(),
            (
                200,
                page(
                    "participant",
                    &[message("2026-11-02T12:05:00Z", "#api", "bob", "it's in")],
                    None,
                ),
            ),
        ]);
        let home = tempfile::tempdir().unwrap();
        // beta's zirc.dev answers nothing: reading it would fail the search.
        let config = format!(
            "web = \"{acme}\"\nowner = \"ada\"\n[server]\nhost = \"acme.irc.zirc.dev\"\n\
             [account]\nname = \"ada-mbp\"\npassword = \"pw\"\n\
             [[org]]\nname = \"beta\"\nweb = \"http://127.0.0.1:9\"\nowner = \"ada\"\n\
             [org.server]\nhost = \"beta.irc.zirc.dev\"\n\
             [org.account]\nname = \"ada-mbp\"\npassword = \"pw\"\n"
        );
        std::fs::write(paths::config(home.path()), config).unwrap();
        let search = |words: &[&str]| {
            let mut argv = vec!["zirc", "search", "--in", "#api", "--json"];
            argv.extend_from_slice(words);
            Cli {
                home: Some(home.path().to_owned()),
                ..Cli::try_parse_from(argv).unwrap()
            }
        };
        let err = run(search(&[]), &mut Vec::new()).await.unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            "this machine is in acme and beta: say which with --org"
        );
        let mut out = Vec::new();
        run(search(&["--org", "acme"]), &mut out).await.unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("it's in"), "{out}");
    }

    #[tokio::test]
    async fn says_why_the_team_has_no_history_to_read() {
        let (web, _served) = zirc_test_ergo::web::serve(vec![
            through_zirc_dev(),
            (
                404,
                r#"{"error":"This organisation's server doesn't serve its history yet."}"#.into(),
            ),
        ]);
        let err = zirc(&web, &["search", "login"]).await.unwrap_err();
        assert!(
            err.contains("This organisation's server doesn't serve its history yet."),
            "{err}"
        );
        // An older zirc.dev, without the token's route either.
        let (web, _served) =
            zirc_test_ergo::web::serve(vec![(404, "Not Found".into()), (404, "Not Found".into())]);
        let err = zirc(&web, &["catchup", "--team"]).await.unwrap_err();
        assert!(
            err.contains("doesn't serve the team's history yet"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn catches_up_on_the_teams_conversations() {
        let overview = serde_json::json!({"policy": "participant", "conversations": [
            {"conversation": "bob", "kind": "dm", "last": "2026-11-02T12:06:00Z", "count": 1},
            {"conversation": "#web", "kind": "channel", "last": "2026-11-02T12:05:00Z", "count": 30},
        ]})
        .to_string();
        let (web, served) = zirc_test_ergo::web::serve(vec![
            through_zirc_dev(),
            (200, overview),
            (
                200,
                page(
                    "participant",
                    &[message("2026-11-02T12:06:00Z", "bob", "bob", "lunch?")],
                    None,
                ),
            ),
            (
                200,
                page(
                    "participant",
                    &[
                        message("2026-11-02T12:05:00Z", "#web", "bob", "ada: review please"),
                        message("2026-11-02T12:04:00Z", "#web", "cy", "deployed"),
                    ],
                    Some("more"),
                ),
            ),
        ]);
        let out = zirc(&web, &["catchup", "--team", "--since", "1h"])
            .await
            .unwrap();
        assert_eq!(
            out,
            "31 said in 2 conversations in the last 1h\n\
             \nnaming you (1):\n  12:05 #web bob: ada: review please [id 1205xyza]\n\
             \ndm with bob (1):\n  12:06 bob: lunch? [id 1206xyza]\n\
             \n#web (30):\n  … 28 earlier\n  12:04 cy: deployed [id 1204xyza]\n  12:05 bob: ada: review please [id 1205xyza]\n"
        );
        let requests = served.join().unwrap();
        assert!(requests[1].starts_with("GET /api/cli/conversations?since="));
        assert!(requests[3].contains("conversation=%23web&limit=20"));
    }
}
