//! What each watch hears, and why: deliveries to watching sessions, what's kept for one
//! that isn't watching, and the reasons a line concerns a session.

use std::collections::{BTreeSet, VecDeque};
use std::ops::ControlFlow;

use tokio::sync::mpsc;
use zirc_core::ipc::{Delivery, Response, Why, Worktree};
use zirc_core::presence::LocalOverlap;
use zirc_core::sessions::LANDING_KIND;
use zirc_core::{decisions, presence};
use zirc_irc::Client;
use zirc_proto::decision::Decision;
use zirc_proto::{Event, Message};

use super::{
    Daemon, MISSED, MISSED_FOR, Said, WATCHER_SESSION, Watched, answered_concerns, channel_of,
    direct_concerns, from_guest, in_repo, landings, lands_under, later, mentions, now, routine,
    wakes,
};

impl Daemon {
    /// Tells every watcher, each with why it concerns the session watching, if it does.
    pub(super) fn tell_watchers(&mut self, response: &Response) {
        // Agents don't hear guests: a guest's line wakes no session and is no session's
        // watch line, even with `--all`. Only a watch with no session, a person's, sees it.
        // (A guest's message to this machine never gets here: guests have no account.) A
        // guest's terminal is the guest's, though: what's said in their room is its sessions'
        // conversation, the guest's own lines among them.
        if self.org.guest.is_none() && from_guest(response) {
            self.watchers.retain(|watcher| {
                watcher.session.is_some()
                    || !matches!(
                        watcher.heard.try_send(response.clone()),
                        Err(mpsc::error::TrySendError::Closed(_))
                    )
            });
            return;
        }
        // An intent's renewal is only noted as told, to whomever it touches: it's no news
        // to anyone, no watch's line, and wakes nobody (zirc_proto::renewal).
        if let Response::Delivery(delivery) = response
            && let Some(intent) = zirc_proto::renewed(&delivery.message.event)
        {
            let mut said = delivery.clone();
            said.message.event = intent;
            self.stale_intent(&Response::Delivery(said));
            return;
        }
        let stale = self.stale_intent(response);
        let mut reasons: Vec<Option<Why>> = self
            .watchers
            .iter()
            .map(|watcher| self.news_why(response, watcher.session.as_deref(), &stale))
            .collect();
        // Told another's branch changes its files, a session waits for it to land.
        self.follow_branches(response, &reasons);
        self.hold_reports(response, &mut reasons);
        self.hold_quiet_intents(response, &mut reasons);
        self.gate_wakes(response, &mut reasons);
        // Before what waited is taken: a state fine again takes back its note that it broke.
        let waking: BTreeSet<String> = self
            .watchers
            .iter()
            .zip(&reasons)
            .filter_map(|(watcher, why)| why.and(watcher.session.clone()))
            .collect();
        self.note_quietly(response, &waking);
        let mut waited = self.waited_for(&reasons);
        let woken: Vec<(String, Why)> = self
            .watchers
            .iter()
            .zip(&reasons)
            .filter(|(watcher, _)| watcher.reachable())
            .filter_map(|(watcher, why)| Some((watcher.session.clone()?, (*why)?)))
            .collect();
        let automatic = wakes::automatic(response);
        let line = wakes::Line::of(response);
        for (session, why) in woken {
            self.woke(&session, why, automatic, line.clone());
            self.heard_now(&session, response);
        }
        self.awaits_heard(response);
        let dressed: Vec<Response> = self
            .watchers
            .iter()
            .zip(&reasons)
            .map(|(watcher, why)| self.dressed(response, *why, watcher.session.as_deref()))
            .collect();
        let mut reasons = reasons.into_iter().zip(dressed);
        // A watcher that can't keep up misses messages rather than stalling the daemon;
        // one that hung up is dropped.
        self.watchers.retain(|watcher| {
            let (why, dressed) = reasons.next().unzip();
            let why = why.flatten();
            if why.is_some()
                && let Some(earlier) = watcher
                    .session
                    .as_ref()
                    .and_then(|session| waited.remove(session))
            {
                for response in earlier {
                    let _ = watcher.heard.try_send(response);
                }
            }
            // A note is for its session's watch alone, even with `--all`.
            if why.is_none()
                && watcher.session.is_some()
                && matches!(response, Response::Direct(direct) if direct.note)
            {
                return !watcher.heard.is_closed();
            }
            !matches!(
                watcher
                    .heard
                    .try_send(dressed.unwrap_or_else(|| response.clone())),
                Err(mpsc::error::TrySendError::Closed(_))
            )
        });
        // Kept for the sessions that have watched, while none of their watches is open.
        // A watch that fell behind and dropped this line doesn't count as one.
        let watching: BTreeSet<&str> = self
            .watchers
            .iter()
            .filter(|watcher| watcher.reachable())
            .filter_map(|watcher| watcher.session.as_deref())
            .collect();
        let away: Vec<String> = self
            .missed
            .keys()
            .filter(|session| !watching.contains(session.as_str()))
            .cloned()
            .collect();
        self.keep_missed(response, &stale, away);
        self.broadcast_told(response);
        self.say_trips();
    }

    /// Keeps `response` for each of the `away` sessions it concerns, to tell them when they
    /// watch again.
    pub(super) fn keep_missed(
        &mut self,
        response: &Response,
        stale: &BTreeSet<String>,
        away: Vec<String>,
    ) {
        let now = self.clock.now();
        for session in away {
            let Some(why) = self.news_why(response, Some(&session), stale) else {
                continue;
            };
            if (why == Why::Mention && later::waits(response))
                || (why == Why::Topic && later::agent_chatter(response))
                || (why == Why::Awaited && later::apple_pass(response))
            {
                self.keep_for_later(&session, response, why);
                continue;
            }
            if why == Why::Intent && self.quiet_intent(response, &session) {
                let dressed = self.dressed(response, Some(why), Some(&session));
                self.keep_for_later(&session, &dressed, why);
                continue;
            }
            let earlier = self.take_later(&session);
            let dressed = self.dressed(response, Some(why), Some(&session));
            let missed = self.missed.entry(session).or_default();
            missed.extend(earlier.into_iter().map(|response| (now, response)));
            missed.retain(|(at, _)| now.duration_since(*at) < MISSED_FOR);
            missed.push_back((now, dressed));
            while missed.len() > MISSED {
                missed.pop_front();
            }
        }
    }

    /// Tells a watch `session` just started what concerned the session while none of its
    /// watches was open, as between one ending and the next starting, between a line before
    /// and a line after.
    pub(super) fn catch_up(&mut self, heard: &mpsc::Sender<Response>, session: &str) {
        let now = self.clock.now();
        let missed: Vec<Response> = self
            .missed
            .insert(session.to_owned(), VecDeque::new())
            .unwrap_or_default()
            .into_iter()
            .filter(|(at, _)| now.duration_since(*at) < MISSED_FOR)
            .map(|(_, response)| response)
            .filter(|response| !self.stale_lease_notice(response))
            .collect();
        if missed.is_empty() {
            return;
        }
        // Answers, reactions and direct messages say no channel: the header goes in the
        // organisation's when nothing kept does. Dropping them for want of one lost a
        // request's answer that came between two watches.
        let channel = missed
            .iter()
            .find_map(channel_of)
            .unwrap_or(zirc_core::project::ORG_CHANNEL)
            .to_owned();
        if self.catch_up_held(session, &missed) {
            return;
        }
        let me = self.client.as_ref().map(|client| client.nick().to_owned());
        let say = |body: String| {
            Response::Delivery(Delivery {
                why: Some(Why::Mention),
                ..Delivery::new(
                    channel.clone(),
                    me.clone().unwrap_or_default(),
                    Message::new(WATCHER_SESSION, Event::Text { body }),
                )
            })
        };
        let _ = heard.try_send(say(format!(
            "{session}: while no watch of yours ran, {} concerned you:",
            match missed.len() {
                1 => "this".to_owned(),
                n => format!("these {n}"),
            }
        )));
        for response in missed {
            let _ = heard.try_send(response);
        }
        let _ = heard.try_send(say(format!("{session}: that's what you missed")));
    }

    /// Which of `paths`, which someone intends to change, touch what `session` on `me` has
    /// changed in `channel`, `mine`, or has said it's about to change.
    pub(super) fn shared_with_mine(
        &self,
        channel: &str,
        me: &str,
        session: Option<&str>,
        mine: &BTreeSet<&str>,
        paths: &[String],
    ) -> Vec<String> {
        let intended: Vec<String> = self
            .presence
            .intents(channel, now())
            .into_iter()
            .filter(|intent| intent.nick == me && session == Some(intent.session.as_str()))
            .flat_map(|intent| intent.paths)
            .collect();
        let mine: Vec<&str> = mine
            .iter()
            .copied()
            .chain(intended.iter().map(String::as_str))
            .collect();
        presence::touching(paths, &mine)
    }

    /// What `session` works on in the repositories of `channel`.
    pub(super) fn session_paths(&self, channel: &str, session: &str) -> BTreeSet<String> {
        self.repos
            .values()
            .filter(|watched| watched.sessions.contains(session))
            .filter(|watched| in_repo(channel, &watched.channel))
            .flat_map(|watched| watched.paths_of(session))
            .collect()
    }

    /// [`Daemon::why`], but for an intent among `stale`'s, which tells the session nothing
    /// new ([`Daemon::stale_intent`]).
    pub(super) fn news_why(
        &self,
        heard: &Response,
        session: Option<&str>,
        stale: &BTreeSet<String>,
    ) -> Option<Why> {
        let why = self.why(heard, session)?;
        let old = matches!(why, Why::Intent) && session.is_some_and(|s| stale.contains(s));
        (!old).then_some(why)
    }

    /// Why `heard` concerns `session`, if it does, so its watcher wakes only for what needs
    /// it: overlaps with its paths, messages to it or this machine, posts that name it, its
    /// paths, or a topic or branch channel it's in, and pushes that change its paths on the
    /// branch it'll rebase onto. Its own posts come back without a reason.
    pub(super) fn why(&self, heard: &Response, session: Option<&str>) -> Option<Why> {
        let me = self.client.as_ref().map(|client| client.nick().to_owned());
        let me = me.as_deref().unwrap_or_default();
        let anyone = session.is_none();
        match heard {
            // In a guest's room, what anyone else says is for every session here.
            Response::Delivery(delivery) if self.org.guest.is_some() => {
                let in_room = self
                    .org
                    .guest
                    .as_ref()
                    .is_some_and(|guest| guest.room.eq_ignore_ascii_case(&delivery.channel));
                let said = matches!(
                    delivery.message.event,
                    Event::Text { .. } | Event::Custom { .. }
                );
                (in_room && said && !delivery.from.eq_ignore_ascii_case(me)).then_some(Why::Direct)
            }
            Response::Delivery(delivery) => self.why_delivered(delivery, me, session),
            Response::Direct(direct) => direct_concerns(direct, session),
            Response::Reaction(reaction) => self.reacted_to(reaction, me, session).or_else(|| {
                session
                    .is_some_and(|session| self.awaits.reaction_wakes(session, reaction))
                    .then_some(Why::ReactionAwaited)
            }),
            Response::Answered(answered) => answered_concerns(answered, session),
            Response::Overlap(overlap) => {
                let mine = self.paths_in(&overlap.channel, session);
                overlap
                    .paths
                    .iter()
                    .any(|path| mine.contains(path.as_str()))
                    .then_some(Why::Overlap)
            }
            // In a shared workspace, only to the session whose branch or claimed changes the
            // push clashes with.
            Response::Conflict(conflict) => (anyone
                || session.is_some_and(|session| self.works_in(&conflict.worktree, session)))
            .then_some(Why::Conflict),
            // Different parts of the same files will merge on their own: a session hears
            // of it only if it turns into the same lines. The person here hears it all.
            Response::LocalOverlap(overlap) => (anyone
                || (!overlap.quiet()
                    && session.is_some_and(|session| self.news_to(overlap, me, session))))
            .then_some(Why::Overlap),
            _ => None,
        }
    }

    /// The repositories `session` works in that `channel` is of; for a watcher that isn't a
    /// session, such as the machine's person, all of them. A session no repository was added
    /// with works in none: it hears what names it, not every worktree's news.
    fn watched_in(&self, channel: &str, session: Option<&str>) -> Vec<&Watched> {
        self.repos
            .values()
            .filter(|watched| session.is_none_or(|s| watched.sessions.contains(s)))
            .filter(|watched| in_repo(channel, &watched.channel))
            .collect()
    }

    /// The paths `session` is changing in `channel`'s repositories; for the person here,
    /// every worktree's.
    fn paths_in(&self, channel: &str, session: Option<&str>) -> BTreeSet<String> {
        match session {
            Some(session) => self.session_paths(channel, session),
            None => self
                .watched_in(channel, None)
                .into_iter()
                .flat_map(|watched| watched.snapshot.dirty.clone())
                .collect(),
        }
    }

    /// Why a message heard in a channel concerns `session`, if it does ([`Self::why`]).
    fn why_delivered(&self, delivery: &Delivery, me: &str, session: Option<&str>) -> Option<Why> {
        // `@everyone` is how a post reaches every session on purpose. The machine and its
        // owner are named by what's for the person here, such as CI saying what it did for
        // them: news to every session on the machine at once, so to none of them.
        // `@everyone` reaches every session only from a post's own words (below): never
        // from a decision's quote, a card, a request or any event that carries text.
        let names: Vec<&str> = match session {
            Some(session) => vec![session],
            None => [Some(me), self.owner.as_deref()]
                .into_iter()
                .flatten()
                .collect(),
        };
        // An agent room's lines are its repository's, but only what names a session in it
        // is news.
        let home = self.home_channel(&delivery.channel);
        let channel = home.as_str();
        if delivery.from == me && session == Some(delivery.message.session.as_str()) {
            return None;
        }
        // This machine's own notices, such as a worktree gone, are for whom they name:
        // their words, a path among them, are no other session's news. One addressed to a
        // session, `cc-b: you've started changing web, where gotchas…`, is that session's
        // alone: the gotchas it passes on name who learnt them, which woke those sessions
        // for another's news (three times in one session, 11 Nov 2026).
        if delivery.from == me
            && delivery.message.session == WATCHER_SESSION
            && let Event::Text { body } = &delivery.message.event
        {
            if let (Some(session), Some((to, _))) = (session, zirc_proto::addressed(body)) {
                return to.eq_ignore_ascii_case(session).then_some(Why::Mention);
            }
            return mentions(body, &names).then_some(Why::Mention);
        }
        // A gotcha is for whoever changes its paths later, when zircd tells it them.
        if let Event::Custom { kind, .. } = &delivery.message.event
            && kind == zirc_core::gotchas::KIND
        {
            return None;
        }
        if let ControlFlow::Break(settled) = self.why_awaited(delivery, channel, session, &names) {
            return settled;
        }
        // CI's landing or deploy for another machine's session is no news to one of ours of
        // the same id: a session's id is unique only on its machine. What's waited on by its
        // branch or build woke it above.
        if session.is_some() && ci_for_another_machine(delivery, me) {
            return None;
        }
        // A workflow saying what the session itself did, took a step on or finished it, is
        // no news to it, as its own posts aren't: a fixer closing three runs at once woke
        // itself six times with them and tripped the wake breaker (14 Nov 2026). What it
        // waits on still wakes it, above.
        if let Some(session) = session
            && let Some(event) = zirc_proto::run::RunEvent::of(&delivery.message.event)
            && event
                .change
                .by()
                .is_some_and(|by| names_session(by, session))
        {
            return None;
        }
        let mine = self.paths_in(channel, session);
        let mine: BTreeSet<&str> = mine.iter().map(String::as_str).collect();
        match &delivery.message.event {
            Event::Text { body } | Event::Custom { text: body, .. } => {
                self.why_said(delivery, body, channel, session, &names, &mine)
            }
            // One that's ended, as a withdrawn one, is nobody's news.
            Event::Intent {
                expires: Some(expires),
                ..
            } if *expires <= now() => None,
            Event::Intent { summary, paths, .. } => {
                if self
                    .shared_with_mine(channel, me, session, &mine, paths)
                    .is_empty()
                {
                    mentions(summary, &names).then_some(Why::Mention)
                } else {
                    Some(Why::Intent)
                }
            }
            Event::Push {
                branch,
                head,
                paths,
                ..
            } => (lands_under(&self.watched_in(channel, session), branch, head, paths)
                || self.partner(delivery, branch, session)
                || session.is_some_and(|session| self.awaits.moves_stack(session, branch, head)))
            .then_some(Why::Landing),
            // Another's commit is news when it's the other side of an overlap, or
            // touches what the session's changing, before it's anywhere near landing.
            Event::Commit { branch, paths, .. } => {
                if self.partner(delivery, branch, session) {
                    Some(Why::Overlap)
                } else {
                    paths
                        .iter()
                        .any(|path| mine.contains(path.as_str()))
                        .then_some(Why::Paths)
                }
            }
            _ => None,
        }
    }

    /// Ends or moves the waits `response` answers: a landing line saying how one ended, a
    /// push moving a branch sessions are stacked on, a state said anew.
    fn awaits_heard(&mut self, response: &Response) {
        // A session here whose landing landed: the intent it declared for it ends.
        if let Response::Delivery(delivery) = response
            && let Event::Custom { kind, text, data } = &delivery.message.event
            && kind == LANDING_KIND
            && text.contains(" landed ")
            && let Some(session) = zirc_proto::ci::Data::of(data).session
        {
            self.intent_landed(&session);
            self.task_landing_landed(&session);
        }
        // Told how it ended, the sessions waiting on it wait no more.
        if let Response::Delivery(delivery) = response
            && let Event::Custom { kind, text, data } = &delivery.message.event
            && kind == LANDING_KIND
            && self
                .awaits
                .heard(text, zirc_proto::ci::Data::of(data).branch.as_deref())
        {
            self.save_awaits();
        }
        // A branch sessions are stacked on moved: their stacks say where to.
        if let Response::Delivery(delivery) = response
            && let Event::Push { branch, head, .. } = &delivery.message.event
            && self.awaits.pushed(branch, head)
        {
            self.save_awaits();
        }
        // Told how a state stands now, the sessions waiting on it changing wait no more.
        if let Response::Delivery(delivery) = response
            && let Some(state) = zirc_proto::state::State::of(&delivery.message.event)
        {
            let at = delivery
                .time
                .as_deref()
                .and_then(zirc_core::time::seconds_at)
                .unwrap_or_else(super::now);
            self.unowned_heard(&state, at);
            self.holds.heard(&state);
            if self.awaits.state_heard(&state) {
                self.save_awaits();
            }
        }
    }

    /// Why a message concerns `session` whatever it says, when it's something the session
    /// asked for or waits on: an answer to its request, a decision for it, a state it waits
    /// on said at another level, or CI on a landing it waits on. Breaks with the answer when
    /// it's one of those, which may be that it isn't this session's.
    fn why_awaited(
        &self,
        delivery: &Delivery,
        channel: &str,
        session: Option<&str>,
        names: &[&str],
    ) -> ControlFlow<Option<Why>> {
        if let Some(asker) = self.asker_replied_to(delivery) {
            return ControlFlow::Break(session.is_none_or(|s| s == asker).then_some(Why::Decision));
        }
        if let Some(decision) = Decision::of(&delivery.message.event) {
            return ControlFlow::Break(
                self.decided_for(&decision, channel, session, names)
                    .then_some(Why::Decision),
            );
        }
        // A state the session waits on, said at another level.
        if let Some(state) = zirc_proto::state::State::of(&delivery.message.event)
            && let Some(session) = session
            && self.awaits.state_changed(session, &state)
        {
            return ControlFlow::Break(Some(Why::StateChanged));
        }
        // CI on a landing the session waits on, however routine for others; but not how its
        // steps stand while it runs (ops/land.sh's step), which needs no one.
        if let Event::Custom { kind, text, data } = &delivery.message.event
            && kind == LANDING_KIND
            && !under_way(&delivery.message, data)
            && let Some(session) = session
            && let Some(why) = self.awaits.concerns(
                session,
                text,
                zirc_proto::ci::Data::of(data).branch.as_deref(),
            )
        {
            return ControlFlow::Break(Some(why));
        }
        ControlFlow::Continue(())
    }

    /// Why what a message says, `body`, concerns `session`: it names it, or a path of
    /// `mine`, or was said where the session listens.
    fn why_said(
        &self,
        delivery: &Delivery,
        body: &str,
        channel: &str,
        session: Option<&str>,
        names: &[&str],
        mine: &BTreeSet<&str>,
    ) -> Option<Why> {
        // What goes as it should, such as a deploy done, isn't news to a
        // channel's every joiner, only to whom it names; and nothing in a
        // landings channel is, which is there for whom it names.
        let topic = self.in_topic(channel, session)
            && !routine(&delivery.from, &delivery.message)
            && !landings(channel);
        if mentions(body, names) || self.broadcast(delivery) {
            Some(Why::Mention)
        } else if self.is_area_room(&delivery.channel) {
            // An area's room: news to whom it names, to a session changing a
            // path it names, and to one that joined the area.
            if mine.iter().any(|path| body.contains(path)) {
                Some(Why::Paths)
            } else {
                (self.in_topic(&delivery.channel, session)
                    && !routine(&delivery.from, &delivery.message))
                .then_some(Why::Topic)
            }
        } else if self.is_post_room(&delivery.channel) {
            // Agents saying what they're doing: news only to whom it names.
            None
        } else if mine.iter().any(|path| body.contains(path)) {
            Some(Why::Paths)
        } else {
            topic.then_some(Why::Topic)
        }
    }

    /// Whether `decision`, heard in `channel`, concerns `session`: the person here hears every
    /// one; a session, those it relayed, those that name it, those for it or a branch it works
    /// on, and all of them once it joined `#decisions` (or `#ops`, where they went before), as
    /// a coordinator does. Waking every session
    /// for each made each spend a turn to say it wasn't theirs.
    pub(super) fn decided_for(
        &self,
        decision: &Decision,
        channel: &str,
        session: Option<&str>,
        names: &[&str],
    ) -> bool {
        let Some(session) = session else { return true };
        decision.session.as_deref() == Some(session)
            || mentions(&decision.quote, names)
            || mentions(&decision.about, names)
            || decision.for_.iter().any(|it| {
                it == session
                    || self.repos.values().any(|watched| {
                        watched.sessions.contains(session)
                            && watched.snapshot.branch.as_deref() == Some(it.as_str())
                    })
            })
            || ([decisions::CHANNEL, decisions::EARLIER].contains(&channel)
                && self.in_topic(channel, Some(session)))
    }

    /// Whether `channel` is a topic `session` joined, or the channel of a branch it works on.
    /// A topic is news to the sessions that joined it, not to every session on the machine.
    pub(super) fn in_topic(&self, channel: &str, session: Option<&str>) -> bool {
        let joined = self.topics.get(channel).is_some_and(|joiners| {
            session.is_none() || joiners.contains(&session.map(str::to_owned))
        });
        joined
            || self
                .repos
                .values()
                .filter(|watched| in_repo(channel, &watched.channel))
                .any(|watched| match session {
                    Some(session) => {
                        watched.sessions.contains(session)
                            && watched.branch_channel().as_deref() == Some(channel)
                    }
                    None => watched.branch_channel().as_deref() == Some(channel),
                })
    }

    /// Whether `overlap` is news to `session` on `me`: between its work and another's, one
    /// side its own and the other not, and not a split it's agreed on.
    pub(super) fn news_to(&self, overlap: &LocalOverlap, me: &str, session: &str) -> bool {
        let (its, others): (Vec<&Worktree>, Vec<&Worktree>) = overlap
            .worktrees
            .iter()
            .partition(|worktree| self.works_in(worktree, session));
        let others = others.len();
        !its.is_empty() && others > 0 && !self.agreed(overlap, me, session)
    }

    /// Whether `session` has agreed on `overlap` with the session on its other side: it said
    /// it's about to change all the overlapping paths, naming that session, as a split two
    /// sessions settled on. The agreement lasts as long as the intent does.
    pub(super) fn agreed(&self, overlap: &LocalOverlap, me: &str, session: &str) -> bool {
        let others: Vec<&str> = overlap
            .worktrees
            .iter()
            .filter(|worktree| !self.works_in(worktree, session))
            .filter_map(|worktree| self.session_of(worktree))
            .collect();
        self.presence
            .intents(&overlap.channel, now())
            .into_iter()
            .filter(|intent| intent.nick == me && intent.session == session)
            .filter(|intent| mentions(&intent.summary, &others))
            .any(|intent| {
                let intended: Vec<&str> = intent.paths.iter().map(String::as_str).collect();
                presence::touching(&overlap.paths, &intended).len() == overlap.paths.len()
            })
    }

    /// The session whose work `worktree`, one of this machine's working sets, is, if known.
    pub(super) fn session_of(&self, worktree: &Worktree) -> Option<&str> {
        self.repos
            .get(&worktree.path)?
            .sets()
            .into_iter()
            .find(|set| set.branch == worktree.branch)?
            .session
    }

    /// Whether `delivery`, a commit or push to `branch`, is by the other side of an overlap
    /// `session` was told of: then its changes are moving, and the session may want to update
    /// from them once they land.
    pub(super) fn partner(&self, delivery: &Delivery, branch: &str, session: Option<&str>) -> bool {
        let Some(session) = session else {
            return false;
        };
        let me = self.client.as_ref().map(Client::nick);
        if Some(delivery.from.as_str()) == me {
            if delivery.message.session == session {
                return false;
            }
            let home = self.home_channel(&delivery.channel);
            return self.told.local.iter().any(|((channel, pair), paths)| {
                // A pair whose overlap was told only as different parts never woke
                // anyone, and its commits needn't either.
                paths.values().any(|(said, _)| *said != Said::Apart)
                    && *channel == home
                    && pair.iter().any(|it| it.branch.as_deref() == Some(branch))
                    && pair.iter().any(|it| {
                        it.branch.as_deref() != Some(branch) && self.works_in(it, session)
                    })
            });
        }
        let mine: Vec<String> = self
            .repos
            .values()
            .filter(|watched| in_repo(&delivery.channel, &watched.channel))
            .flat_map(|watched| watched.paths_of(session))
            .collect();
        self.told
            .remote
            .get(&(self.home_channel(&delivery.channel), delivery.from.clone()))
            .is_some_and(|told| mine.iter().any(|path| told.contains_key(path)))
    }

    /// Whether `worktree`, one of this machine's working sets, is `session`'s.
    pub(super) fn works_in(&self, worktree: &Worktree, session: &str) -> bool {
        self.repos
            .get(&worktree.path)
            .is_some_and(|watched| watched.is_of(session))
    }
}

/// Whether `delivery` is CI's landing or deploy for a session on a machine other than `me`,
/// as its data's `machine` says. An older CI's event names no machine: never another's.
fn ci_for_another_machine(delivery: &Delivery, me: &str) -> bool {
    let Event::Custom { kind, data, .. } = &delivery.message.event else {
        return false;
    };
    (kind == zirc_proto::ci::LANDING_KIND || kind == zirc_proto::ci::DEPLOY_KIND)
        && zirc_proto::ci::Data::of(data)
            .machine
            .is_some_and(|machine| !machine.eq_ignore_ascii_case(me))
}

/// Whether `name`, as a workflow says who did something or who a step is for, is
/// `session`: the session alone, or as `machine/session`.
pub(super) fn names_session(name: &str, session: &str) -> bool {
    name == session
        || name
            .rsplit_once('/')
            .is_some_and(|(_, name)| name == session)
}

/// Whether a landing's event, `message` with its `data`, only says how its steps stand while
/// it runs: a live update ([`Message::is_live`]), or its start, running with its steps. A
/// wait on the landing is for how it ends.
fn under_way(message: &Message, data: &serde_json::Value) -> bool {
    let data = zirc_proto::ci::Data::of(data);
    message.is_live() || (data.state.as_deref() == Some("running") && !data.steps.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CI's landing for `session` on `machine`, or naming no machine, as an older CI's does.
    fn landing(machine: Option<&str>) -> Delivery {
        let mut data = serde_json::json!({ "session": "cc-1", "state": "passed" });
        if let Some(machine) = machine {
            data["machine"] = machine.into();
        }
        let event = Event::Custom {
            kind: zirc_proto::ci::LANDING_KIND.into(),
            data,
            text: "cc-1: landed web as abc1234".into(),
        };
        Delivery::new("#api", zirc_proto::ci::NICK, Message::new("ci", event))
    }

    #[test]
    fn a_landing_names_its_sessions_machine() {
        assert!(ci_for_another_machine(
            &landing(Some("bob-laptop")),
            "me-mbp"
        ));
        assert!(!ci_for_another_machine(&landing(Some("ME-MBP")), "me-mbp"));
        // An older CI names no machine, or an empty one: never another's.
        assert!(!ci_for_another_machine(&landing(None), "me-mbp"));
        assert!(!ci_for_another_machine(&landing(Some("")), "me-mbp"));
    }
}
