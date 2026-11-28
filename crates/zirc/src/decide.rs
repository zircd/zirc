//! `zirc decide`, `zirc decisions` and `zirc request`: the organisation's decisions and the owner's answers.

use std::io::{IsTerminal as _, Write};
use std::path::Path;

use anyhow::{Result, bail};
use clap::Args;
use zirc_core::decisions;
use zirc_core::ipc::{self, Delivery, Heard, Request, Response};
use zirc_proto::decision::Decision;
use zirc_proto::request::Answer;

use crate::args::{OrgArg, moment_arg, words};
use crate::client::{ask_in, here, org_here, waits};
use crate::lines::{SHORT_ID, id_of, one_line, said};

/// Which decisions `zirc decisions` shows.
pub(crate) struct Look {
    /// Only those about this.
    pub(crate) about: Option<String>,
    /// Only those since this moment, in seconds since the Unix epoch.
    pub(crate) since: Option<u64>,
    /// How many of #decisions's and #ops's newest messages to look through.
    pub(crate) limit: usize,
    /// Those later ones replaced too.
    pub(crate) all: bool,
}

/// The decisions among the newest `limit` messages of #decisions and of #ops, where they went
/// before, oldest first.
pub(crate) async fn decisions_read(
    socket: &Path,
    org: Option<&str>,
    limit: usize,
) -> Result<Vec<Delivery>> {
    let mut found = Vec::new();
    for target in [decisions::CHANNEL, decisions::EARLIER] {
        let request = Request::Read {
            target: target.to_owned(),
            limit,
            since: None,
            until: None,
        };
        let Response::Messages { messages } = waits(socket, org, request).await? else {
            return Err(crate::client::unexpected());
        };
        found.extend(
            messages
                .into_iter()
                .filter(|delivery| Decision::of(&delivery.message.event).is_some()),
        );
    }
    // Times as the server says them, RFC 3339 in UTC, sort as text.
    found.sort_by(|a, b| a.time.cmp(&b.time));
    Ok(found)
}

/// The whole id of the decision whose id starts with `id`, as `zirc decisions` shows it,
/// among the recent ones of `org` (on a machine in several organisations): one, or it says
/// why not.
pub(crate) async fn decision_id(socket: &Path, org: Option<&str>, id: &str) -> Result<String> {
    let found: Vec<String> = decisions_read(socket, org, 500)
        .await?
        .into_iter()
        .filter_map(|delivery| delivery.id)
        .filter(|full| full.starts_with(id))
        .collect();
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => bail!("no decision {id} among the recent ones: `zirc decisions` shows their ids"),
        _ => bail!("several decisions' ids start with {id}: give more of it"),
    }
}

/// The ids of the decisions in `found` that a later one replaced.
pub(crate) fn replaced(found: &[Delivery]) -> std::collections::BTreeSet<String> {
    found
        .iter()
        .filter_map(|delivery| Decision::of(&delivery.message.event)?.replaces)
        .collect()
}

pub(crate) async fn look_up(
    socket: &Path,
    org: OrgArg,
    look: Look,
    json: bool,
    out: &mut impl Write,
) -> Result<()> {
    let org = org_here(socket, org).await?;
    let messages = decisions_read(socket, org.as_deref(), look.limit).await?;
    let replaced = replaced(&messages);
    let after = look.since;
    let about = look.about.map(|about| about.to_lowercase());
    let found = messages.iter().filter(|delivery| {
        let current = look.all || delivery.id.as_ref().is_none_or(|id| !replaced.contains(id));
        current
            && Decision::of(&delivery.message.event).is_some_and(|decision| {
                about
                    .as_ref()
                    .is_none_or(|about| decision.about.to_lowercase().contains(about))
            })
            && after.is_none_or(|after| {
                delivery
                    .time
                    .as_deref()
                    .and_then(zirc_core::time::seconds_at)
                    .is_some_and(|at| at >= after)
            })
    });
    let mut any = false;
    for delivery in found {
        any = true;
        if json {
            writeln!(out, "{}", serde_json::to_string(delivery)?)?;
        } else {
            let when = delivery.time.as_deref().unwrap_or("?");
            let gone = if delivery.id.as_ref().is_some_and(|id| replaced.contains(id)) {
                " (replaced)"
            } else {
                ""
            };
            writeln!(
                out,
                "{when} {}{gone}{}",
                one_line(&said(delivery)),
                id_of(delivery.id.as_deref())
            )?;
        }
    }
    if !any && !json {
        writeln!(out, "no decisions found in {}", decisions::CHANNEL)?;
    }
    Ok(())
}

/// Asks `request` for `session`, and prints its id, then with `wait` its answer once it
/// comes.
pub(crate) async fn request_answer(
    socket: &Path,
    org: Option<&str>,
    session: &str,
    request: Request,
    wait: bool,
    json: bool,
    out: &mut impl Write,
) -> Result<()> {
    // Watching before asking, so the answer can't come before the watch is open.
    let mut watching = if wait {
        Some(ipc::watch(socket, Some(session)).await?)
    } else {
        None
    };
    let said = match &request {
        Request::AskOwner { yes, no, .. } => (yes.clone(), no.clone()),
        _ => (String::new(), String::new()),
    };
    let asks_owner = matches!(request, Request::AskOwner { .. });
    let Response::Requested { id, channel, taken } = ask_in(socket, org, request).await? else {
        return Err(crate::client::unexpected());
    };
    if json {
        writeln!(
            out,
            "{}",
            serde_json::json!({ "id": id, "channel": channel, "taken": taken })
        )?;
    } else {
        writeln!(out, "asked in {channel} as {id}")?;
        // zirc can't see what the person types in the agent's own tool: an answer there
        // leaves this in front of them until it's withdrawn.
        if asks_owner {
            writeln!(
                out,
                "if they answer in your own tool instead, withdraw it: zirc request --session {session} --withdraw {id}"
            )?;
        }
    }
    // Taken as recommended, as this machine is set to: nothing to wait for.
    if let Some(answer) = taken {
        if !json {
            writeln!(out, "{}", taken_line(answer, &said.0, &said.1))?;
        }
        return Ok(());
    }
    out.flush()?;
    let Some(watching) = &mut watching else {
        return Ok(());
    };
    while let Some(heard) = watching.next().await? {
        if let Heard::Answered(answered) = heard
            && answered.request.id == id
        {
            if json {
                writeln!(out, "{}", serde_json::to_value(&answered)?)?;
            } else {
                writeln!(out, "{}", one_line(&answered_line(&answered)))?;
            }
            return Ok(());
        }
    }
    bail!("zircd stopped before {id} was answered: `zirc decisions` shows it once it is")
}

/// What `zirc request` says when its recommendation was taken at once: to go ahead with what
/// that answer does, `yes` or `no` as the request said them, and that its user can still say
/// otherwise.
fn taken_line(answer: Answer, yes: &str, no: &str) -> String {
    let does = if answer == Answer::No { no } else { yes };
    // What the answer does is the request's own words, which may or may not end a sentence.
    let does = one_line(does);
    let does = does.trim().trim_end_matches(['.', ';', ':']);
    format!(
        "taken as you recommended ({}): {does}. Go ahead; your user sees it in #decisions, and \
         your watch says if they answer otherwise",
        answer.said()
    )
}

/// An answer to a request as a line: `#decisions mattias answered yes (👍) to mbp/3: Add
/// a CI node?`, asks to know more about it, or left it to the session (🤷).
pub(crate) fn answered_line(answered: &ipc::Answered) -> String {
    let request = &answered.request;
    let said = match answered.answer {
        Answer::More => "asks to know more",
        Answer::Yes => "answered yes",
        Answer::No => "answered no",
        Answer::Yours => "left it to you",
    };
    let about = if answered.answer == Answer::More {
        "about"
    } else {
        "to"
    };
    let next = match answered.answer {
        Answer::More => " (tell them more with zirc dm)",
        // The person's word to go ahead as the session judges best, and to say what it did.
        Answer::Yours => " (your call: do what you judge best, and say what you chose)",
        Answer::Yes | Answer::No => "",
    };
    format!(
        "{} {} {said} ({}) {about} {}: {}{next}",
        zirc_proto::request::CHANNEL,
        answered.by,
        answered.emoji,
        request.id,
        request.question
    )
}

// `zirc decide`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct DecideArgs {
    /// What they said, word for word.
    #[arg(long)]
    pub(crate) quote: String,
    /// What it covers, in a few words, to look it up by.
    #[arg(long)]
    pub(crate) about: String,
    /// Where to read more: the conversation, a document, an issue.
    #[arg(long)]
    pub(crate) link: Option<String>,
    /// The session it was said to. Defaults to $ZIRC_SESSION; without one, it's yours
    /// at a terminal.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: Option<String>,
    /// A session or branch it's for, whose watch it wakes; again for more. Besides
    /// those, it wakes only the session relaying it, sessions it names, and sessions
    /// that joined #ops; the rest find it with `zirc decisions`.
    #[arg(long = "for", value_name = "SESSION|BRANCH")]
    pub(crate) for_: Vec<String>,
    /// The earlier decision this takes the place of, by the id `zirc decisions` shows:
    /// it then decides nothing more, and `zirc decisions` shows only this one.
    #[arg(long, value_name = "ID")]
    pub(crate) replaces: Option<String>,
    /// Post it even though the quote is a question, or the session relayed the same quote
    /// within the hour.
    #[arg(long)]
    pub(crate) force: bool,
    #[command(flatten)]
    pub(crate) org: OrgArg,
}

/// Why `zirc decide` refuses a quote that asks rather than decides.
const QUESTION_REFUSED: &str = "not decided: the quote is a question, so it isn't their \
    decision yet. Quote their answer to it, or ask them with zirc request; or post it as it \
    is with --force";

/// How long the same quote relayed again counts as the same decision.
const REPEAT_SECONDS: u64 = 60 * 60;

/// Whether `quote` asks rather than decides: it ends with a question mark. One decision
/// quoted "Can we make web deployment automatic …?" and went further than it asked (the
/// chat analysis, 11 Nov 2026).
fn asks(quote: &str) -> bool {
    quote.trim_end().ends_with('?')
}

/// The id of a decision in `found` that `session` relayed with the same `quote` within
/// [`REPEAT_SECONDS`] of `now`: one promotion was decided twice, six minutes apart, on the
/// same "Yes, push it" (the chat analysis, 11 Nov 2026).
fn repeats(found: &[Delivery], session: &str, quote: &str, now: u64) -> Option<String> {
    found.iter().rev().find_map(|delivery| {
        let decision = Decision::of(&delivery.message.event)?;
        let at = zirc_core::time::seconds_at(delivery.time.as_deref()?)?;
        (decision.session.as_deref() == Some(session)
            && decision.quote.trim() == quote.trim()
            && now.saturating_sub(at) < REPEAT_SECONDS)
            .then(|| delivery.id.clone())
            .flatten()
    })
}

// `zirc decisions`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct DecisionsArgs {
    /// Only those about this: words in what they cover.
    #[arg(long)]
    pub(crate) about: Option<String>,
    /// Only those in the last 30m, 12h, 3d or 30d.
    #[arg(long)]
    pub(crate) since: Option<String>,
    /// How many of #decisions's and #ops's newest messages to look through.
    #[arg(short = 'n', long, default_value_t = 500)]
    pub(crate) limit: usize,
    /// Those later decisions replaced too.
    #[arg(long)]
    pub(crate) all: bool,
    /// Print JSON: a line for each decision.
    #[arg(long)]
    pub(crate) json: bool,
    #[command(flatten)]
    pub(crate) org: OrgArg,
}

// `zirc request`'s arguments: their doc comments are its --help.
#[derive(Debug, Args)]
pub(crate) struct RequestArgs {
    /// The question, one line.
    #[arg(required_unless_present = "withdraw", num_args = 1..)]
    pub(crate) question: Vec<String>,
    /// What a yes does.
    #[arg(long, required_unless_present = "withdraw")]
    pub(crate) yes: Option<String>,
    /// What a no does.
    #[arg(long, required_unless_present = "withdraw")]
    pub(crate) no: Option<String>,
    /// Take back a request of yours that's still waiting, by its id (`mbp/12`), as you no
    /// longer need the answer: it leaves your user's list. Finishing your task does it too.
    #[arg(long, value_name = "ID", conflicts_with_all = ["question", "yes", "no", "recommend", "irreversible", "wait"])]
    pub(crate) withdraw: Option<String>,
    /// The answer you recommend, shown first.
    #[arg(long, value_parser = ["yes", "no"])]
    pub(crate) recommend: Option<String>,
    /// Acting on a yes can't be undone: the yes reaches you only after a minute in which
    /// a 👎 stops it.
    #[arg(long)]
    pub(crate) irreversible: bool,
    /// Wait for the answer, and print it.
    #[arg(long)]
    pub(crate) wait: bool,
    /// Print JSON: the request, then with --wait the answer.
    #[arg(long)]
    pub(crate) json: bool,
    /// Who's asking: an agent session's id. Defaults to $ZIRC_SESSION.
    #[arg(long, env = "ZIRC_SESSION")]
    pub(crate) session: String,
    #[command(flatten)]
    pub(crate) org: OrgArg,
}

/// `zirc decide`.
pub(crate) async fn decide(socket: &Path, args: DecideArgs, out: &mut impl Write) -> Result<()> {
    let DecideArgs {
        quote,
        about,
        link,
        session,
        for_,
        replaces,
        force,
        org,
    } = args;
    if asks(&quote) && !force {
        bail!("{QUESTION_REFUSED}");
    }
    // Without a session, it's the person's own: only one at a terminal says so.
    if session.is_none() && !std::io::stdin().is_terminal() {
        bail!(
            "say which session it was said to with --session; only a person at a \
             terminal posts a decision as their own"
        );
    }
    // The organisation the decision goes to, where the one it replaces is looked up too.
    let org = org_here(socket, org).await?;
    let replaces = match replaces {
        Some(id) => Some(decision_id(socket, org.as_deref(), &id).await?),
        None => None,
    };
    if let Some(session) = session.as_deref()
        && replaces.is_none()
        && !force
    {
        let found = decisions_read(socket, org.as_deref(), 200).await?;
        if let Some(id) = repeats(&found, session, &quote, crate::catchup::now()) {
            let id = id.get(..SHORT_ID).unwrap_or(&id);
            bail!(
                "not decided: {session} relayed the same quote within the hour, as {id}. If \
                 this one says it better, add --replaces {id}; or post it as well with --force"
            );
        }
    }
    let request = Request::Decide {
        repo: here(None)?,
        session,
        quote,
        about,
        link,
        for_,
        replaces,
    };
    let Response::Decided { channels } = ask_in(socket, org.as_deref(), request).await? else {
        return Err(crate::client::unexpected());
    };
    writeln!(out, "decided, in {}", channels.join(" and "))?;
    Ok(())
}

/// `zirc decisions`: the decisions people made ([`decisions_read`]), about `about` if given,
/// in the last `since` if given, among the newest `limit` messages.
pub(crate) async fn decisions(
    socket: &Path,
    args: DecisionsArgs,
    out: &mut impl Write,
) -> Result<()> {
    let DecisionsArgs {
        about,
        since,
        limit,
        all,
        json,
        org,
    } = args;
    let since = since
        .as_deref()
        .map(|it| moment_arg(it, "--since"))
        .transpose()?;
    let look = Look {
        about,
        since,
        limit,
        all,
    };
    look_up(socket, org, look, json, out).await
}

/// `zirc request`.
pub(crate) async fn request(socket: &Path, args: RequestArgs, out: &mut impl Write) -> Result<()> {
    let RequestArgs {
        question,
        yes,
        no,
        recommend,
        irreversible,
        withdraw,
        wait,
        json,
        session,
        org,
    } = args;
    let org = org_here(socket, org).await?;
    if let Some(id) = withdraw {
        let request = Request::Withdraw {
            session,
            id: id.clone(),
        };
        let Response::Done = ask_in(socket, org.as_deref(), request).await? else {
            return Err(crate::client::unexpected());
        };
        if json {
            writeln!(out, "{}", serde_json::json!({ "withdrawn": id }))?;
        } else {
            writeln!(out, "withdrew {id}: it no longer waits for an answer")?;
        }
        return Ok(());
    }
    let request = Request::AskOwner {
        session: session.clone(),
        question: words(&question)?,
        yes: yes.unwrap_or_default(),
        no: no.unwrap_or_default(),
        recommend: recommend.map(|it| if it == "no" { Answer::No } else { Answer::Yes }),
        irreversible,
    };
    request_answer(socket, org.as_deref(), &session, request, wait, json, out).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decided(session: &str, quote: &str, id: &str, at: u64) -> Delivery {
        let decision = Decision {
            by: "ada".into(),
            quote: quote.into(),
            about: "the release".into(),
            session: Some(session.into()),
            link: None,
            for_: Vec::new(),
            replaces: None,
        };
        let message = zirc_proto::Message::new(session, decision.event());
        serde_json::from_value(serde_json::json!({
            "channel": decisions::CHANNEL,
            "from": "alpha",
            "message": message,
            "id": id,
            "time": zirc_core::time::tag(at),
        }))
        .unwrap()
    }

    #[test]
    fn a_question_isnt_a_decision_yet() {
        assert!(asks("Can we make web deployment automatic?"));
        assert!(asks("ship it? "));
        assert!(!asks("Yes, push it"));
        assert!(!asks("Header inbox popover"));
    }

    #[test]
    fn the_same_quote_relayed_again_within_the_hour_repeats_it() {
        let now = 1_800_000_000;
        let found = [decided("cc-1", "Yes, push it", "first", now - 360)];
        assert_eq!(
            repeats(&found, "cc-1", " Yes, push it", now).as_deref(),
            Some("first")
        );
        // Another session's, another quote, or an older one doesn't.
        assert_eq!(repeats(&found, "cc-2", "Yes, push it", now), None);
        assert_eq!(repeats(&found, "cc-1", "Yes, bump it", now), None);
        assert_eq!(repeats(&found, "cc-1", "Yes, push it", now + 3600), None);
    }

    #[test]
    fn a_request_taken_at_once_says_what_to_go_ahead_with() {
        for yes in ["deploys it now", "deploys it now."] {
            assert_eq!(
                taken_line(Answer::Yes, yes, "leaves it"),
                "taken as you recommended (yes): deploys it now. Go ahead; your user sees it in \
                 #decisions, and your watch says if they answer otherwise"
            );
        }
        assert!(
            taken_line(Answer::No, "x", "leaves it")
                .starts_with("taken as you recommended (no): leaves it. Go ahead")
        );
    }
}
