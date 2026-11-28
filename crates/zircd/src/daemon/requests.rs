//! The daemon's part in requests ([`zirc_proto::request`]): it asks this machine's owner in
//! `#decisions` for its sessions, keeps each open request and its message's id across
//! restarts, and takes the owner's reaction to it as their answer, which it tells the
//! session that asked and shows on the request's card.
//!
//! Whose a reaction is, is the server's word alone: its `account-tag`. A reaction answers a
//! request only when that account is the request's owner and it's on the request's own
//! message, as this machine's echo of it said, while the request is open. A nick is never
//! anyone's word (anyone can take one on a server that lets people in without an account),
//! and nor is a reaction with no account, or one from this machine's own account, which
//! its sessions react as: a session can't answer for its person.
//!
//! A yes to what can't be undone is held for [`Daemon::request_hold`] before it counts, and
//! told to no one meanwhile: a 👎, a ❓ or taking the 👍 back stops it. A ❓ is answered by
//! the session in replies under the request ([`Daemon::explain`]); the owner's own replies
//! there reach that session, and their reaction on one of them answers as one on the
//! request does.
//!
//! A request nobody needs answered any more is withdrawn ([`State::Withdrawn`]), which its
//! card says and every app's "Waiting for you" drops it for: when its session takes it back
//! (`zirc request --withdraw`), when its session finishes or drops its task, a landing's
//! `done:` included, and when its session is gone, away as long as its row stays listed (the
//! user's choices, 24 Nov 2026).

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use zirc_core::ipc::{Answered, Delivery, Reaction, Response};
use zirc_core::paths;
use zirc_irc::Line;
use zirc_proto::request::{self, Answer, CHANNEL, Request, State, Withdrawal};
use zirc_proto::session::AWAY_FOLD_SECS;

use super::{Ask, Daemon, read_saved};
use crate::activity;

/// The requests this machine has asked and not had a yes or no to, saved across restarts.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct Requests {
    /// By id.
    open: BTreeMap<String, Open>,
    /// Counts the requests made, to name each apart.
    made: u64,
}

impl Requests {
    /// Whether any request is waiting for its answer.
    pub(super) fn any_open(&self) -> bool {
        !self.open.is_empty()
    }
}

/// A request waiting for its answer.
#[derive(Debug, Serialize, Deserialize)]
struct Open {
    request: Request,
    /// The id the server gave its message, once this machine's echo of it said: until then
    /// no reaction can be on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    msgid: Option<String>,
    /// For a held yes, when it counts, in seconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    held_until: Option<u64>,
    /// The ids of the session's replies under it, which the owner can react to or answer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    replies: Vec<String>,
    /// For one taken as recommended ([`Daemon::take_recommended`]): until when, in seconds
    /// since the Unix epoch, the owner's reaction on it still says otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    taken_until: Option<u64>,
    /// When it was asked, in seconds since the Unix epoch: none for one asked before zircd
    /// kept it, which counts as long ago.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    asked_at: Option<u64>,
}

impl Open {
    /// Whether it still waits on its owner: not one taken as recommended, which is kept only
    /// for the owner to say otherwise.
    fn waits(&self) -> bool {
        self.taken_until.is_none() && self.request.waits()
    }

    /// Whether the message `id` is the request's own, or one of the session's replies
    /// under it.
    fn says(&self, id: &str) -> bool {
        self.msgid.as_deref() == Some(id) || self.replies.iter().any(|reply| reply == id)
    }
}

/// How long a yes to an irreversible request is held, unless config.toml's `request_hold`
/// sets it (tests do).
pub(super) const HOLD: Duration = Duration::from_secs(request::HOLD_SECONDS);

/// How long the owner's reaction on a request taken as recommended still says otherwise, in
/// seconds: a day, by when its session has long acted on it.
const OVERTURNS_FOR: u64 = 86_400;

/// What the request a reaction is on asks for, once it's been checked.
enum Outcome {
    /// It answers the request.
    Answered(Box<Answered>),
    /// It takes back a "tell me more" or a held yes, leaving the request open again; or
    /// holds a yes to what can't be undone. Either way, only the card says so.
    Restated(Box<Request>),
    /// The owner gave the answer a request taken as recommended already had: it's theirs
    /// now, and closed, and its session, which acted on it already, isn't told again.
    Agreed(Box<Request>),
}

impl Daemon {
    /// Takes in the requests saved before zircd last stopped.
    pub(super) fn restore_requests(&mut self) {
        if let Some(requests) = read_saved::<Requests>(&paths::requests(&self.org.state)) {
            self.requests = requests;
        }
    }

    fn save_requests(&self) {
        let json = serde_json::to_string_pretty(&self.requests).expect("requests serialize");
        if let Err(err) = paths::write_private(&paths::requests(&self.org.state), &json) {
            tracing::warn!("couldn't save the requests: {err}");
        }
    }

    /// Asks this machine's owner `question` for `session` in `#decisions`.
    pub(super) fn ask_owner(
        &mut self,
        session: &str,
        question: String,
        yes: String,
        no: String,
        recommend: Option<Answer>,
        irreversible: bool,
    ) -> Result<Response> {
        let by = self
            .owner
            .clone()
            .context("this machine has no owner to ask: zirc login sets one")?;
        for (what, flag) in [(&question, "the question"), (&yes, "--yes"), (&no, "--no")] {
            ensure!(!what.trim().is_empty(), "say {flag}");
        }
        let client = self.connected()?;
        let nick = client.nick().to_owned();
        if !self.joined.contains(CHANNEL) {
            client.send(Line::new("JOIN", [CHANNEL]))?;
        }
        self.requests.made += 1;
        let id = format!("{nick}/{}", self.requests.made);
        let request = Request {
            recommend,
            irreversible,
            ..Request::open(id.clone(), session, by, question, yes, no)
        };
        // A request that couldn't be sent is said so, never as asked.
        let message = self.signed(session, request.event());
        self.publish_out(CHANNEL, message, None, None)
            .context("the request wasn't sent")?;
        // Taken as recommended at once, on a machine set to, when a yes can be undone: the
        // owner chose it for most of some two hundred go-aheads a day (12 Nov 2026).
        let taken = recommend.filter(|answer| {
            *answer != Answer::More
                && !irreversible
                && self
                    .config
                    .ask(|settings| Some(crate::config::accept_recommended(settings)))
                    .unwrap_or(false)
        });
        let mut open = Open {
            request,
            msgid: None,
            held_until: None,
            replies: Vec::new(),
            taken_until: None,
            asked_at: Some(super::now()),
        };
        if let Some(answer) = taken {
            self.take_recommended(&mut open, answer);
        }
        let who = activity::who(session, self.name_of(session).as_deref());
        self.activity
            .say(&activity::asked(&who, &open.request.by, &id));
        self.requests.open.insert(id.clone(), open);
        self.save_requests();
        Ok(Response::Requested {
            id,
            channel: CHANNEL.to_owned(),
            taken,
        })
    }

    /// Takes `open`'s request as its session recommended, `answer`, and says so on its card:
    /// the command that asked tells the session, so no watch wakes for it. It stays among
    /// the open ones for [`OVERTURNS_FOR`], as the owner's reaction on it still counts.
    fn take_recommended(&mut self, open: &mut Open, answer: Answer) {
        let now = super::now();
        open.request.state = State::Answered {
            answer,
            emoji: String::new(),
            at: zirc_core::time::tag(now),
            recommended: true,
        };
        open.taken_until = Some(now + OVERTURNS_FOR);
        let session = open.request.session.clone();
        self.publish(CHANNEL, &session, open.request.event());
    }

    /// Withdraws the requests `ids`, still waiting, saying `why` on each one's card: no
    /// answer to them counts from here on.
    fn withdraw(&mut self, ids: &[String], why: Withdrawal) {
        let at = zirc_core::time::tag(super::now());
        for id in ids {
            let Some(open) = self.requests.open.remove(id) else {
                continue;
            };
            let request = Request {
                state: State::Withdrawn {
                    at: at.clone(),
                    why,
                },
                ..open.request
            };
            self.publish(CHANNEL, &request.session, request.event());
            let who = activity::who(&request.session, self.name_of(&request.session).as_deref());
            self.activity.say(&activity::withdrawn(&who, id, why));
        }
        if !ids.is_empty() {
            self.save_requests();
        }
    }

    /// `session` takes back its request `id` (`zirc request --withdraw`): only its own, and
    /// only while it waits.
    pub(super) fn withdraw_request(&mut self, session: &str, id: &str) -> Result<Response> {
        let open = self
            .requests
            .open
            .get(id)
            .filter(|open| open.waits())
            .with_context(|| format!("no request {id} is waiting for an answer"))?;
        ensure!(
            open.request.session == session,
            "{id} is {}'s request, not {session}'s",
            open.request.session
        );
        self.withdraw(&[id.to_owned()], Withdrawal::Asked);
        Ok(Response::Done)
    }

    /// Withdraws what `session` asked and still waits for, as its task is finished or
    /// dropped: nobody acts on the answer.
    pub(super) fn withdraw_requests_of(&mut self, session: &str) {
        let ids: Vec<String> = self
            .requests
            .open
            .iter()
            .filter(|(_, open)| open.waits() && open.request.session == session)
            .map(|(id, _)| id.clone())
            .collect();
        self.withdraw(&ids, Withdrawal::Done);
    }

    /// Withdraws the waiting requests whose sessions are gone at `now`: asked
    /// [`AWAY_FOLD_SECS`] ago or more by a session that hasn't had a watch open for as long,
    /// so its row is folded away and nobody is there to act on an answer.
    fn withdraw_gone(&mut self, now: u64) {
        let ids: Vec<String> = self
            .requests
            .open
            .iter()
            .filter(|(_, open)| {
                open.waits()
                    && now.saturating_sub(open.asked_at.unwrap_or(0)) >= AWAY_FOLD_SECS
                    && !self.live.knows(&open.request.session)
            })
            .map(|(id, _)| id.clone())
            .collect();
        self.withdraw(&ids, Withdrawal::Gone);
    }

    /// Notes the id the server gave a request this machine asked, as its echo says: the
    /// message a reaction must be on to answer it.
    pub(super) fn request_echoed(&mut self, channel: &str, msgid: &str, event: &zirc_proto::Event) {
        let Some(said) = Request::of(event) else {
            return;
        };
        // Only the first, open posting: the card posted again for an answer isn't the
        // message to react to.
        if channel != CHANNEL || said.state != State::Open {
            return;
        }
        if let Some(open) = self.requests.open.get_mut(&said.id)
            && open.msgid.is_none()
        {
            open.msgid = Some(msgid.to_owned());
            self.save_requests();
        }
        // The command that asked is answered now: the server has it ([`Daemon::requested`]).
        if let Some(at) = self
            .asking
            .iter()
            .position(|asking| matches!(&asking.what, Ask::Requested { id, .. } if *id == said.id))
        {
            let asking = self.asking.remove(at);
            if let Ask::Requested { response, .. } = asking.what {
                let _ = asking.answer.send(*response);
            }
        }
    }

    /// Notes the id of a request's message whose echo this machine never heard, as its
    /// connection dropped just as it asked: `found` is the message with that id, read from
    /// the server's history because its owner reacted to it ([`Daemon::fetched`]). Without
    /// it no reaction was ever on the request, which stayed open with nobody told (a user's
    /// review, 22 Nov 2026). Only what the server says this machine's own account sent,
    /// asking just what the open request asks, counts: never a nick, nor a card someone
    /// else posted under its id. Whether it was one.
    pub(super) fn request_found(&mut self, msgid: &str, found: &Delivery) -> bool {
        let Some(said) = Request::of(&found.message.event) else {
            return false;
        };
        let ours = found.account.as_deref().is_some_and(|account| {
            self.account
                .as_deref()
                .is_some_and(|mine| mine.eq_ignore_ascii_case(account))
        });
        if !ours || found.channel != CHANNEL || said.state != State::Open {
            return false;
        }
        let Some(open) = self.requests.open.get_mut(&said.id) else {
            return false;
        };
        let asked = Request {
            state: State::Open,
            ..open.request.clone()
        };
        if open.msgid.is_some() || asked != said {
            return false;
        }
        open.msgid = Some(msgid.to_owned());
        self.save_requests();
        true
    }

    /// Takes `reaction` as its owner's answer to a request, if it's one. Returns whether it
    /// was on an open request, which then says what it did, rather than the reaction.
    pub(super) fn request_reacted(&mut self, reaction: &Reaction) -> bool {
        let Some(id) = self.requests.open.iter().find_map(|(id, open)| {
            (reaction.channel == CHANNEL && open.says(&reaction.to)).then(|| id.clone())
        }) else {
            return false;
        };
        match self.outcome(&id, reaction) {
            Some(Outcome::Answered(answered)) => {
                if answered.answer != Answer::More {
                    self.requests.open.remove(&id);
                }
                self.save_requests();
                self.publish(CHANNEL, &answered.request.session, answered.request.event());
                self.tell_watchers(&Response::Answered(answered));
                true
            }
            Some(Outcome::Restated(request)) => {
                self.save_requests();
                self.publish(CHANNEL, &request.session, request.event());
                true
            }
            Some(Outcome::Agreed(request)) => {
                self.requests.open.remove(&id);
                self.save_requests();
                self.publish(CHANNEL, &request.session, request.event());
                true
            }
            None => false,
        }
    }

    /// What `reaction` does to the open request `id`, if anything: only the server's word
    /// that the request's owner gave it counts.
    fn outcome(&mut self, id: &str, reaction: &Reaction) -> Option<Outcome> {
        let open = self.requests.open.get_mut(id)?;
        let msgid = open.msgid.clone()?;
        let request = &mut open.request;
        // The account-tag, and nothing else: never the nick, which anyone can take, and
        // never a reaction without an account.
        let account = reaction.account.as_deref()?;
        if !account.eq_ignore_ascii_case(&request.by) {
            return None;
        }
        let answer = request::answer_of(&reaction.emoji)?;
        // A shrug says nothing to a request that can't be undone, which a person decides.
        if answer == Answer::Yours && !request.offers_yours() {
            return None;
        }
        // Taken as recommended: the owner agreeing changes nothing but the card, and a
        // reaction taken back says nothing new. Anything else is their answer, as below.
        if let State::Answered {
            answer: took,
            recommended: true,
            ..
        } = &request.state
        {
            if reaction.removed {
                return None;
            }
            if *took == answer {
                let at = reaction
                    .time
                    .clone()
                    .unwrap_or_else(|| zirc_core::time::tag(super::now()));
                request.state = State::Answered {
                    answer,
                    emoji: reaction.emoji.clone(),
                    at,
                    recommended: false,
                };
                open.taken_until = None;
                return Some(Outcome::Agreed(Box::new(request.clone())));
            }
        }
        if reaction.removed {
            // Taking back a yes or no can't happen: either closed the request. A "tell me
            // more" or a held yes taken back leaves it open again.
            let was = match &request.state {
                State::Answered { answer, .. } => *answer,
                State::Held { .. } => Answer::Yes,
                State::Open | State::Withdrawn { .. } => return None,
            };
            if was != answer {
                return None;
            }
            request.state = State::Open;
            open.held_until = None;
            return Some(Outcome::Restated(Box::new(request.clone())));
        }
        let at = reaction
            .time
            .clone()
            .unwrap_or_else(|| zirc_core::time::tag(super::now()));
        if answer == Answer::Yes && request.irreversible {
            // Held, and told to no one, until it counts.
            if matches!(request.state, State::Held { .. }) {
                return None;
            }
            let until = super::now() + self.request_hold.as_secs();
            open.held_until = Some(until);
            request.state = State::Held {
                emoji: reaction.emoji.clone(),
                at,
                until: zirc_core::time::tag(until),
            };
            return Some(Outcome::Restated(Box::new(request.clone())));
        }
        open.held_until = None;
        open.taken_until = None;
        request.state = State::Answered {
            answer,
            emoji: reaction.emoji.clone(),
            at: at.clone(),
            recommended: false,
        };
        Some(Outcome::Answered(Box::new(Answered {
            request: request.clone(),
            answer,
            by: account.to_owned(),
            emoji: reaction.emoji.clone(),
            at,
            msgid,
            org: None,
            why: None,
        })))
    }

    /// Counts each held yes whose hold is over: the request is answered yes, and its session
    /// told. Called every second.
    pub(super) fn requests_tick(&mut self) {
        let now = super::now();
        // What was taken as recommended is past saying otherwise: no longer kept.
        let before = self.requests.open.len();
        self.requests
            .open
            .retain(|_, open| open.taken_until.is_none_or(|until| until > now));
        if self.requests.open.len() < before {
            self.save_requests();
        }
        self.withdraw_gone(now);
        let due: Vec<String> = self
            .requests
            .open
            .iter()
            .filter(|(_, open)| open.held_until.is_some_and(|until| until <= now))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            let Some(open) = self.requests.open.remove(&id) else {
                continue;
            };
            let mut request = open.request;
            let State::Held { emoji, .. } = request.state else {
                continue;
            };
            let at = zirc_core::time::tag(now);
            request.state = State::Answered {
                answer: Answer::Yes,
                emoji: emoji.clone(),
                at: at.clone(),
                recommended: false,
            };
            self.save_requests();
            self.publish(CHANNEL, &request.session, request.event());
            let answered = Answered {
                by: request.by.clone(),
                request,
                answer: Answer::Yes,
                emoji,
                at,
                msgid: open.msgid.unwrap_or_default(),
                org: None,
                why: None,
            };
            self.tell_watchers(&Response::Answered(Box::new(answered)));
        }
    }

    /// Notes `session`'s reply `msgid` under a request, as this machine's echo of it says,
    /// so the owner's reaction or reply to it counts as one to the request.
    pub(super) fn request_reply_echoed(
        &mut self,
        channel: &str,
        msgid: &str,
        to: &str,
        session: &str,
    ) {
        if channel != CHANNEL {
            return;
        }
        if let Some(open) = self
            .requests
            .open
            .values_mut()
            .find(|open| open.msgid.as_deref() == Some(to) && open.request.session == session)
        {
            open.replies.push(msgid.to_owned());
            self.save_requests();
        }
    }

    /// The session that asked, when `delivery` is the owner's own reply under its open
    /// request: then it concerns that session (and the person's own watch) alone.
    pub(super) fn asker_replied_to(&self, delivery: &Delivery) -> Option<&str> {
        if delivery.channel != CHANNEL {
            return None;
        }
        let to = delivery.reply.as_deref()?;
        let account = delivery.account.as_deref()?;
        self.requests
            .open
            .values()
            .find(|open| open.says(to) && account.eq_ignore_ascii_case(&open.request.by))
            .map(|open| open.request.session.as_str())
    }
}
