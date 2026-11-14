//! Requests that wait on the server: trims, history reads, conversation lists and posts' echoes, and the
//! batches that answer them.

use tokio::sync::oneshot;
use zirc_core::history::{MAX_PAGES, Pages};
use zirc_core::ipc::{Request, Response};
use zirc_core::{history, trim};
use zirc_irc::Line;

use super::NOT_CONNECTED;
use super::{Ask, Asking, Daemon, Input, heard, now, vouch};

/// How long the same post from the same session counts as said again ([`Daemon::post`]).
const REPEAT_SECONDS: u64 = 60;

impl Daemon {
    /// Asks the server to trim message `id` from `target`, answering once it says.
    pub(super) fn trim(
        &mut self,
        target: String,
        id: String,
        reason: Option<&str>,
        answer: oneshot::Sender<Response>,
    ) {
        if target.eq_ignore_ascii_case(zirc_core::states::CHANNEL) {
            self.drop_ops();
        }
        let sent = match &self.client {
            None => Err(NOT_CONNECTED.to_owned()),
            Some(client) if !client.has_cap(trim::CAP) => Err(format!(
                "{} doesn't let messages be trimmed from its history",
                self.server
            )),
            Some(client) => client
                .send(trim::request(&target, &id, reason))
                .map_err(|err| err.to_string()),
        };
        match sent {
            Ok(()) => self.wait(answer, Ask::Trim { target, id }),
            Err(message) => {
                let _ = answer.send(Response::Error { message });
            }
        }
    }

    /// Asks the server for the history `read` ([`Request::Read`]) asks for, answering once
    /// it's all in.
    pub(super) fn read(&mut self, read: Request, answer: oneshot::Sender<Response>) {
        let Request::Read {
            target,
            limit,
            since,
            until,
        } = read
        else {
            return;
        };
        let pages = Pages::new(history::Range {
            since,
            until,
            limit: Some(limit.max(1)),
        });
        let sent = match &self.client {
            None => Err(NOT_CONNECTED.to_owned()),
            Some(client) if !client.has_cap("draft/chathistory") || !client.has_cap("batch") => {
                Err(format!("{} keeps no history to read", self.server))
            }
            // A channel's history is its members': one this machine isn't in is joined
            // first, once the server says it has it.
            Some(client) if target.starts_with('#') && !self.joined.contains(&target) => {
                match client.send(Line::new("LIST", [target.as_str()])) {
                    Ok(()) => {
                        let found = false;
                        self.wait(
                            answer,
                            Ask::Listing {
                                target,
                                pages,
                                found,
                            },
                        );
                    }
                    Err(err) => {
                        let message = err.to_string();
                        let _ = answer.send(Response::Error { message });
                    }
                }
                return;
            }
            Some(client) => pages
                .request(&target)
                .map_or(Ok(()), |request| client.send(request))
                .map_err(|err| err.to_string()),
        };
        match sent {
            Ok(()) => self.wait(answer, Ask::reading(target, pages)),
            Err(message) => {
                let _ = answer.send(Response::Error { message });
            }
        }
    }

    /// Asks the server which private conversations had messages in the last `seconds`
    /// (`CHATHISTORY TARGETS`), answering `answer` once it says.
    pub(super) fn conversations(&mut self, seconds: u64, answer: oneshot::Sender<Response>) {
        let until = now();
        let from = until.saturating_sub(seconds);
        let sent = match &self.client {
            None => Err(NOT_CONNECTED.to_owned()),
            Some(client) if !client.has_cap("draft/chathistory") || !client.has_cap("batch") => {
                Err(format!("{} keeps no history to read", self.server))
            }
            Some(client) => client
                .send(Line::new(
                    "CHATHISTORY",
                    [
                        "TARGETS",
                        &format!("timestamp={}", zirc_core::time::tag(from)),
                        &format!("timestamp={}", zirc_core::time::tag(until + 60)),
                        "100",
                    ],
                ))
                .map_err(|err| err.to_string()),
        };
        match sent {
            Ok(()) => self.wait(
                answer,
                Ask::Conversations {
                    batch: None,
                    targets: Vec::new(),
                },
            ),
            Err(message) => {
                let _ = answer.send(Response::Error { message });
            }
        }
    }

    /// Keeps `answer` until the server says what `what` waits for, or until it's waited
    /// long enough.
    pub(super) fn wait(&mut self, answer: oneshot::Sender<Response>, what: Ask) {
        let token = self.next_token;
        self.next_token += 1;
        self.asking.push(Asking {
            token,
            answer,
            what,
        });
        self.expire_after(token);
    }

    /// Answers the request waiting on `line`, if one is. Whether the line was the request's
    /// alone: history read for a client isn't news.
    pub(super) fn answers(&mut self, line: &Line) -> bool {
        if self.asking.is_empty() {
            return false;
        }
        let param = |i: usize| line.params.get(i).map_or("", String::as_str);
        if let Some(outcome) = trim::outcome(line) {
            let waiting = self.asking.iter().position(|asking| {
                matches!(&asking.what, Ask::Trim { target, id } if outcome.is_about(target, id))
            });
            if let Some(at) = waiting {
                let response = match &outcome {
                    trim::Outcome::Trimmed { .. } => Response::Done,
                    trim::Outcome::Refused { text, .. } => Response::Error {
                        message: format!("the server wouldn't trim it: {text}"),
                    },
                };
                self.finish(at, response);
            }
            return false;
        }
        match line.command.as_str() {
            // ERR_NOSUCHNICK, for a private conversation's target.
            "401" => {
                let nick = param(1);
                let waiting = self
                    .asking
                    .iter()
                    .position(|asking| asking.what.target().eq_ignore_ascii_case(nick));
                if let Some(at) = waiting {
                    let message = format!("there's no {nick} on the server");
                    self.finish(at, Response::Error { message });
                }
                false
            }
            "FAIL" if param(0) == "CHATHISTORY" => {
                let failed = self.asking.iter().position(|asking| {
                    matches!(&asking.what, Ask::Read { batch: None, .. } | Ask::Reacted { batch: None, .. }
                        if line.params.iter().any(|p| p.eq_ignore_ascii_case(asking.what.target())))
                });
                if let Some(at) = failed
                    && matches!(self.asking[at].what, Ask::Reacted { .. })
                {
                    self.fetched(at);
                } else if let Some(at) = failed {
                    let text = line.params.last().map_or("", String::as_str);
                    let message = format!("the server wouldn't give its history: {text}");
                    self.finish(at, Response::Error { message });
                }
                failed.is_some()
            }
            // RPL_LIST, a channel the server has, for a read waiting to join it.
            "322" => {
                let listing = self
                    .asking
                    .iter_mut()
                    .find_map(|asking| match &mut asking.what {
                        Ask::Listing { target, found, .. }
                            if target.eq_ignore_ascii_case(param(1)) =>
                        {
                            Some(found)
                        }
                        _ => None,
                    });
                listing.is_some_and(|found| {
                    *found = true;
                    true
                })
            }
            // RPL_LISTEND: the server answers LISTs in order, so the first read waiting on
            // one.
            "323" => {
                let Some(at) = self
                    .asking
                    .iter()
                    .position(|asking| matches!(asking.what, Ask::Listing { .. }))
                else {
                    return false;
                };
                self.listed(at);
                true
            }
            // RPL_ISON, for a workflow's file waiting on whether the server runs workflows.
            "303" => {
                let Some(at) = self
                    .asking
                    .iter()
                    .position(|asking| matches!(asking.what, Ask::Hosted { .. }))
                else {
                    return false;
                };
                let on = param(1)
                    .split_whitespace()
                    .any(|nick| nick.eq_ignore_ascii_case(zirc_proto::workflow::HOST));
                self.hosted(at, on);
                true
            }
            "BATCH" => self.history_batch(param(0), param(1), param(2)),
            _ => self.read_line(line),
        }
    }

    /// The server listed what a read waiting at `at` asked about: it joins the channel and
    /// reads its history if the server has it, and is refused if not.
    pub(super) fn listed(&mut self, at: usize) {
        let Ask::Listing {
            target,
            pages,
            found,
        } = &self.asking[at].what
        else {
            return;
        };
        let (target, pages, found) = (target.clone(), pages.clone(), *found);
        if !found {
            let message =
                format!("{target} isn't on the server: not joining it, which would make it");
            self.finish(at, Response::Error { message });
            return;
        }
        let sent = self.client.as_ref().map_or_else(
            || Err(NOT_CONNECTED.to_owned()),
            |client| {
                client
                    .send(Line::new("JOIN", [target.as_str()]))
                    .and_then(|()| {
                        pages
                            .request(&target)
                            .map_or(Ok(()), |request| client.send(request))
                    })
                    .map_err(|err| err.to_string())
            },
        );
        match sent {
            Ok(()) => {
                self.asking[at].what = Ask::reading(target, pages);
                // Its request went out now, after any sent since it listed.
                self.rearm(at);
            }
            Err(message) => self.finish(at, Response::Error { message }),
        }
    }

    /// A batch starting (`+reference`) or ending (`-reference`): the history a read waits
    /// for, of `target`, when its `kind` is `chathistory`. Whether it was.
    pub(super) fn history_batch(&mut self, reference: &str, kind: &str, target: &str) -> bool {
        if let Some(reference) = reference.strip_prefix('+') {
            if kind == "draft/chathistory-targets" {
                let starting = self
                    .asking
                    .iter_mut()
                    .find_map(|asking| match &mut asking.what {
                        Ask::Conversations { batch, .. } if batch.is_none() => Some(batch),
                        _ => None,
                    });
                return starting.is_some_and(|batch| {
                    *batch = Some(reference.to_owned());
                    true
                });
            }
            if kind != "chathistory" {
                return false;
            }
            // The server answers in the order it was asked, and each page's request takes
            // a new token: the batch is the read's whose request went out first. Two reads
            // of one channel at once, as sessions' `zirc who`s make, once took each other's
            // pages, till one waited past its time ([`super::batches`]).
            let starting = self
                .asking
                .iter_mut()
                .filter_map(|asking| match &mut asking.what {
                    Ask::Read {
                        target: of, batch, ..
                    } if batch.is_none() && of.eq_ignore_ascii_case(target) => {
                        Some((asking.token, batch))
                    }
                    Ask::Reacted {
                        reaction, batch, ..
                    } if batch.is_none() && reaction.channel.eq_ignore_ascii_case(target) => {
                        Some((asking.token, batch))
                    }
                    _ => None,
                })
                .min_by_key(|(token, _)| *token)
                .map(|(_, batch)| batch);
            starting.is_some_and(|batch| {
                *batch = Some(reference.to_owned());
                true
            })
        } else if let Some(reference) = reference.strip_prefix('-') {
            let targets = self.asking.iter().position(|asking| {
                matches!(&asking.what, Ask::Conversations { batch: Some(b), .. } if b == reference)
            });
            if let Some(at) = targets
                && let Ask::Conversations { targets, .. } = &mut self.asking[at].what
            {
                let targets = std::mem::take(targets);
                self.finish(at, Response::Conversations { targets });
                return true;
            }
            let reacted = self.asking.iter().position(
                |asking| matches!(&asking.what, Ask::Reacted { batch: Some(b), .. } if b == reference),
            );
            if let Some(at) = reacted {
                self.fetched(at);
                return true;
            }
            let done = self.asking.iter_mut().position(
                |asking| matches!(&asking.what, Ask::Read { batch: Some(b), .. } if b == reference),
            );
            if let Some(at) = done {
                self.paged(at);
            }
            done.is_some()
        } else {
            false
        }
    }

    /// A line of history a read waits for, kept when it's a message. Whether it was one.
    pub(super) fn read_line(&mut self, line: &Line) -> bool {
        let Some(batch) = line.tag("batch") else {
            return false;
        };
        if let Some(Ask::Conversations { targets, .. }) =
            self.asking.iter_mut().map(|asking| &mut asking.what).find(
                |what| matches!(what, Ask::Conversations { batch: Some(b), .. } if b == batch),
            )
        {
            // CHATHISTORY TARGETS <target> <timestamp>
            if line.command == "CHATHISTORY"
                && line.params.first().is_some_and(|it| it == "TARGETS")
                && let Some(target) = line.params.get(1)
            {
                targets.push(target.clone());
            }
            return true;
        }
        if let Some(Ask::Reacted {
            reaction, found, ..
        }) = self
            .asking
            .iter_mut()
            .map(|asking| &mut asking.what)
            .find(|what| matches!(what, Ask::Reacted { batch: Some(b), .. } if b == batch))
        {
            if line.command == "PRIVMSG"
                && line.tag("msgid") == Some(reaction.to.as_str())
                && let Some((delivery, _)) = heard(&reaction.channel, line)
            {
                *found = Some(Box::new(delivery));
            }
            return true;
        }
        let zirc_dev = self.zirc_dev;
        let Some(Ask::Read {
            target, seen, page, ..
        }) = self
            .asking
            .iter_mut()
            .map(|asking| &mut asking.what)
            .find(|what| matches!(what, Ask::Read { batch: Some(b), .. } if b == batch))
        else {
            return false;
        };
        // Joins, parts, quits and reactions are history too, replayed as the lines they
        // were (event-playback): a page of them alone isn't history's start.
        seen.push(history::Seen::of(line));
        if line.command == "PRIVMSG"
            && let Some((mut delivery, _)) = heard(target, line)
        {
            vouch(&mut delivery, zirc_dev);
            page.push((seen.len().saturating_sub(1), delivery));
        }
        true
    }

    /// A page of history in for the read waiting at `at`: it asks for the page before, or,
    /// with the range all in, answers with what it kept.
    pub(super) fn paged(&mut self, at: usize) {
        let Ask::Read {
            target,
            batch,
            seen,
            page,
            messages,
            pages,
        } = &mut self.asking[at].what
        else {
            return;
        };
        // The items older than the range are its first `older`: so are their messages.
        let older = pages.page(seen);
        seen.clear();
        let kept = page
            .drain(..)
            .filter(|(place, _)| *place >= older)
            .map(|(_, delivery)| delivery);
        messages.splice(0..0, kept);
        *batch = None;
        let next = pages.request(target);
        if let Some(request) = next
            && let Some(client) = &self.client
            && client.send(request).is_ok()
        {
            // Each page has its own time to come.
            self.rearm(at);
            return;
        }
        let Ask::Read {
            messages, pages, ..
        } = &mut self.asking[at].what
        else {
            return;
        };
        if pages.stopped_short() {
            tracing::info!("a read stopped after {MAX_PAGES} pages of history");
        }
        let mut messages = std::mem::take(messages);
        messages.drain(..pages.excess(messages.len()));
        self.finish(at, Response::Messages { messages });
    }

    /// Sends what [`Request::Workflow`] asks to: a workflow's file once the server says its
    /// workflows' host is on (`ISON`), anything else at once.
    pub(super) async fn workflow(&mut self, request: Request, answer: oneshot::Sender<Response>) {
        let Request::Workflow {
            repo,
            session,
            channel,
            event,
        } = request
        else {
            return;
        };
        if zirc_proto::workflow::Deploy::of(&event).is_some() {
            self.deploy(&repo, session, &channel, event, answer).await;
            return;
        }
        let response = self
            .workflow_send(&repo, &session, &channel, event)
            .await
            .map_or_else(
                |err| Response::Error {
                    message: format!("{err:#}"),
                },
                Response::channel,
            );
        let _ = answer.send(response);
    }

    /// Sends a workflow's file ([`Request::Workflow`]) once the server says its workflows'
    /// host is on (`ISON`). Workflows are on only for the organisations zirc.dev turns them on
    /// for (`spec.workflows`): elsewhere nothing is sent, so `#workflows` isn't even made.
    async fn deploy(
        &mut self,
        repo: &std::path::Path,
        session: String,
        channel: &str,
        event: zirc_proto::Event,
        answer: oneshot::Sender<Response>,
    ) {
        if let Err(err) = self.workflow_checked(repo, channel, &event).await {
            let message = format!("{err:#}");
            let _ = answer.send(Response::Error { message });
            return;
        }
        let asked = self
            .connected()
            .and_then(|client| Ok(client.send(Line::new("ISON", [zirc_proto::workflow::HOST]))?));
        match asked {
            Ok(()) => self.wait(
                answer,
                Ask::Hosted {
                    session,
                    event: Box::new(event),
                },
            ),
            Err(err) => {
                let message = format!("{err:#}");
                let _ = answer.send(Response::Error { message });
            }
        }
    }

    /// The server said whether its workflows' host is `on`, for the file waiting at `at`: it's
    /// sent if it is.
    fn hosted(&mut self, at: usize, on: bool) {
        let Ask::Hosted { session, event } = &self.asking[at].what else {
            return;
        };
        let (session, event) = (session.clone(), (**event).clone());
        let response = if on {
            self.workflow_emit(&session, zirc_proto::workflow::CHANNEL, event)
                .map_or_else(
                    |err| Response::Error {
                        message: format!("{err:#}"),
                    },
                    Response::channel,
                )
        } else {
            Response::Error {
                message: format!(
                    "{} doesn't run workflows: they're on only for the organisations zirc.dev \
                     has turned them on for. Nothing was sent.",
                    self.server
                ),
            }
        };
        self.finish(at, response);
    }

    /// Fails the request waiting with `token` once it's waited [`Daemon::ask_wait`]. With
    /// none, as a test may set, it fails before anything the server says next.
    fn expire_after(&self, token: u64) {
        let inputs = self.inputs.clone();
        if self.ask_wait.is_zero() {
            let _ = inputs.send(Input::Expired(token));
            return;
        }
        let wait = self.ask_wait;
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            let _ = inputs.send(Input::Expired(token));
        });
    }

    /// Gives the request waiting at `at` its whole time to be answered again.
    pub(super) fn rearm(&mut self, at: usize) {
        let token = self.next_token;
        self.next_token += 1;
        self.asking[at].token = token;
        self.expire_after(token);
    }

    /// Posts as `post` ([`Request::Post`], [`Request::SealedPost`]) asks, answering once the server's echo gives the
    /// post its link ([`Daemon::links_posts`]), or at once without one.
    /// The same post from the same session within [`REPEAT_SECONDS`] isn't said again: one
    /// session's `done:` line went out twice in the same second (the chat analysis, 11 Nov
    /// 2026), waking everyone it named twice.
    pub(super) async fn post(&mut self, post: Request, answer: oneshot::Sender<Response>) {
        let (Request::Post { session, text, .. } | Request::SealedPost { session, text, .. }) =
            &post
        else {
            return;
        };
        let session = session.clone();
        let at = now();
        if let Some((last, when)) = self.last_posts.get(&session)
            && last == text
            && at.saturating_sub(*when) < REPEAT_SECONDS
        {
            let _ = answer.send(Response::Error {
                message: format!(
                    "not posted: {session} posted the same {}s ago; it went out once",
                    at.saturating_sub(*when)
                ),
            });
            return;
        }
        let text = text.clone();
        let response = self.respond(post).await;
        // Only a post that went out counts: one refused may be asked again at once.
        if let Ok(Response::Channel { .. }) = &response {
            self.last_posts.insert(session.clone(), (text, at));
        }
        match response {
            Ok(Response::Channel { channel, .. }) if self.links_posts() => {
                self.wait(answer, Ask::Echo { channel, session });
            }
            response => {
                let _ = answer.send(response.unwrap_or_else(|err| Response::Error {
                    message: format!("{err:#}"),
                }));
            }
        }
    }

    /// The request waiting with `token` waited too long: a post went out all the same, and is
    /// told without its link; the rest fail.
    pub(super) fn expired(&mut self, token: u64) {
        let Some(at) = self.asking.iter().position(|a| a.token == token) else {
            return;
        };
        let response = match &self.asking[at].what {
            Ask::Reacted { .. } => return self.fetched(at),
            Ask::Echo { channel, .. } => Response::channel(channel.clone()),
            _ => Response::Error {
                message: format!(
                    "the server didn't answer within {}s",
                    self.ask_wait.as_secs()
                ),
            },
        };
        self.finish(at, response);
    }

    /// The server answered, or didn't in time, for the message the reaction waiting at `at` is
    /// on: the reaction is told now, with what it's on if the server gave it. The message is
    /// remembered as heard either way, by its id and channel, so a session can reply under it
    /// or react to it by the start of its id.
    pub(super) fn fetched(&mut self, at: usize) {
        let Ask::Reacted {
            reaction, found, ..
        } = self.asking.remove(at).what
        else {
            return;
        };
        let mut reaction = *reaction;
        match found {
            Some(delivery) => {
                let session = Some(delivery.message.session.as_str()).filter(|it| !it.is_empty());
                let about = zirc_core::ipc::Reacted::of(&delivery);
                self.recent.heard(
                    &reaction.to,
                    &reaction.channel,
                    &delivery.from,
                    session,
                    Some(about.clone()),
                );
                reaction.on = Some(about);
            }
            None => self
                .recent
                .heard(&reaction.to, &reaction.channel, "", None, None),
        }
        self.tell_reaction(reaction);
    }

    /// Answers the request waiting at `at` with `response`.
    pub(super) fn finish(&mut self, at: usize, response: Response) {
        let asking = self.asking.remove(at);
        if let Ask::Read { target, pages, .. } | Ask::Listing { target, pages, .. } = &asking.what {
            self.read_in(target, pages.range(), &response);
        }
        let _ = asking.answer.send(response);
    }
}
