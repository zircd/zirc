//! Who's working on what: the working sets this machine says in each channel, and the
//! overlaps it finds between them.

use std::collections::BTreeSet;
use std::time::Instant;

use zirc_core::ipc::{ChannelPresence, Member, Response, Worktree};
use zirc_core::presence::LocalOverlap;
use zirc_core::project::ORG_CHANNEL;
use zirc_irc::Line;
use zirc_proto::telemetry as usage;
use zirc_proto::telemetry::Event as Usage;
use zirc_proto::{AGENTS_CHANNEL, Event};

use super::book::Field;
use super::{Daemon, Input, PART_ROOM, WATCHER_SESSION, now, predict, worktree_name};

impl Daemon {
    /// Everyone's working sets in the repositories' scopes, each of this machine's
    /// worktrees on its own.
    pub(super) fn who(&self) -> Response {
        let nick = self.client.as_ref().map(|client| client.nick().to_owned());
        let scopes = self.scopes();
        let channels = self
            .joined
            .iter()
            .filter(|channel| scopes.contains(*channel))
            .map(|channel| {
                let mut members = self.presence.members(channel);
                if let Some(nick) = &nick {
                    members.extend(
                        self.local(channel)
                            .into_iter()
                            .filter(|(_, _, paths)| !paths.is_empty())
                            .map(|(worktree, session, paths)| Member {
                                nick: nick.clone(),
                                name: worktree_name(&worktree.path),
                                branch: worktree.branch.clone(),
                                session_name: session
                                    .as_deref()
                                    .and_then(|session| self.name_of(session)),
                                session_tool: session
                                    .as_deref()
                                    .and_then(|session| self.book.get(Field::Tool, session)),
                                session_avatar: session
                                    .as_deref()
                                    .and_then(|session| self.book.get(Field::Avatar, session)),
                                session,
                                worktree: Some(worktree),
                                paths: paths.into_iter().collect(),
                            }),
                    );
                }
                members.sort_by(|a, b| (&a.nick, &a.worktree).cmp(&(&b.nick, &b.worktree)));
                let mut built = self.presence.built(channel);
                if let Some(nick) = &nick {
                    let here = self.sets_said(channel).into_iter();
                    built.extend(here.filter_map(|((_, branch), said)| said.built(nick, branch)));
                }
                ChannelPresence {
                    channel: channel.clone(),
                    org: None,
                    members,
                    claims: self.presence.claims(channel),
                    intents: self.presence.intents(channel, now()),
                    landings: self.presence.landings(channel),
                    built,
                }
            })
            .collect();
        Response::Who { channels }
    }

    /// This machine's working sets in `channel`, each with where it is, whose it is and its
    /// paths: one for each worktree.
    pub(super) fn local(&self, channel: &str) -> Vec<(Worktree, Option<String>, BTreeSet<String>)> {
        self.repos
            .iter()
            .filter(|(_, watched)| watched.scope == channel)
            .flat_map(|(root, watched)| {
                watched.sets().into_iter().map(|set| {
                    let worktree = Worktree {
                        path: root.clone(),
                        branch: set.branch,
                    };
                    // Its own commits that haven't landed are its work as much as what's
                    // uncommitted.
                    let paths = set.paths.union(&set.committed).cloned().collect();
                    (worktree, set.session.map(str::to_owned), paths)
                })
            })
            .collect()
    }

    /// This machine's work in progress in the repositories that share `channel`.
    pub(super) fn mine(&self, channel: &str) -> BTreeSet<String> {
        self.local(channel)
            .into_iter()
            .flat_map(|(_, _, paths)| paths)
            .collect()
    }

    /// Tells `channel` what this machine is working on once [`Daemon::announce_every`] has
    /// passed since it last did, if that's changed: each announcement is a line for each
    /// working set, and a machine with many sessions changes something every few seconds.
    /// Meanwhile each worktree whose set changed is said alone, sooner (`worktree_updates`).
    pub(super) fn announce_soon(&mut self, channel: &str) {
        self.to_announce.insert(channel.to_owned());
        self.worktree_updates.changed(channel);
    }

    /// Someone joined `channel`, this machine itself if `ours`: whoever it was should hear
    /// what we're working on, at once when it's us, and when the next announcement's due
    /// when it's someone else, as the apps join and leave all the time. `#agents` is every
    /// watched repository's: this machine in it is in each one's scope.
    ///
    /// When `who` is one of zirc's integrations, only which sessions are live is said again
    /// (the workflows' host reads roles from it): they post or read history and go, and
    /// never read working sets, intents or leases. CI joins for each line it says, and saying
    /// all of it again for each, some 370 lines here every 8 seconds, was more than Ergo lets
    /// a connection send: it held every line this machine sent back by minutes (24 Nov 2026).
    pub(super) fn someone_joined(&mut self, channel: &str, who: &str, ours: bool) {
        // Which sessions are live is out of history too.
        self.live.say_again(channel);
        if !ours && zirc_proto::names::is_integration_nick(who) {
            return;
        }
        if ours {
            self.joined.insert(channel.to_owned());
            if self.invite_owner.remove(channel)
                && let (Some(owner), Some(client)) = (&self.owner, &self.client)
            {
                let _ = client.send(Line::new("INVITE", [owner.as_str(), channel]));
            }
            // Back after a gap: what was said meanwhile.
            self.replay_joined(channel);
            self.keep_ops(channel);
        } else if channel == ORG_CHANNEL {
            // Lease events are out of history: one joining hears this machine's again.
            self.say_leases_again();
        }
        if channel != AGENTS_CHANNEL {
            return;
        }
        for scope in self.scopes() {
            if ours {
                self.joined.insert(scope.clone());
            }
            // So are zircd's own intents.
            self.say_auto_intents_again(&scope);
            if self.mine(&scope).is_empty() {
                continue;
            }
            if ours {
                self.announce(&scope, true, self.clock.now());
            } else {
                self.last_announced.remove(&scope);
                self.announce_soon(&scope);
            }
        }
    }

    /// Announces in the channels due, those whose time has come.
    pub(super) fn announce_due(&mut self, now: Instant) {
        self.update_due(now);
        let due: Vec<String> = self
            .to_announce
            .iter()
            .filter(|channel| {
                self.last_announced
                    .get(*channel)
                    .is_none_or(|(at, _)| now.duration_since(*at) >= self.announce_every)
            })
            .cloned()
            .collect();
        for channel in due {
            self.to_announce.remove(&channel);
            self.announce(&channel, false, now);
        }
    }

    /// Tells `channel` what this machine is working on: each worktree's set in turn, the
    /// first replacing what the channel knew and the rest adding to it, or an empty set.
    ///
    /// Unless `again`, as for someone who's just joined, not when it would say what it said
    /// there last. `now` is when it's said, from which the next waits [`Daemon::announce_every`].
    pub(super) fn announce(&mut self, channel: &str, again: bool, now: Instant) {
        let said = self.sets_said(channel);
        let mut sets: Vec<(String, Event)> = said
            .clone()
            .into_iter()
            .map(|(place, said)| {
                let session = said.session().to_owned();
                // Room for the real numbers, which are known once the parts are.
                (session, said.event(place, true, [PART_ROOM, PART_ROOM]))
            })
            .collect();
        if sets.is_empty() {
            let nothing = Event::Working {
                paths: Vec::new(),
                continued: false,
                worktree: None,
                branch: None,
                part: Some([1, 1]),
                base: None,
                committed: Vec::new(),
            };
            sets.push((WATCHER_SESSION.to_owned(), nothing));
        }
        // Every part fits a line, so readers can count them.
        let parts: Vec<(String, Event)> = sets
            .into_iter()
            .flat_map(|(session, event)| {
                zirc_core::send::fitting_as(&self.signed(&session, event.clone()), event)
                    .into_iter()
                    .map(move |event| (session.clone(), event))
            })
            .collect();
        if !again
            && self
                .last_announced
                .get(channel)
                .is_some_and(|(_, said)| *said == parts)
        {
            return;
        }
        self.last_announced
            .insert(channel.to_owned(), (now, parts.clone()));
        let count = u32::try_from(parts.len()).unwrap_or(u32::MAX);
        for (i, (session, mut event)) in parts.into_iter().enumerate() {
            if let Event::Working {
                continued, part, ..
            } = &mut event
            {
                let number = u32::try_from(i + 1).unwrap_or(u32::MAX);
                *continued = number > 1;
                *part = Some([number, count]);
            }
            self.publish(channel, &session, event);
        }
        for (_, said) in said {
            self.publish_lines(channel, said.session(), &said);
        }
        self.told_whole(channel);
    }

    /// Tells watching clients about overlaps in `channel` they haven't heard about yet:
    /// with other machines now, and between this machine's own worktrees once Git has said
    /// which lines they share, in a task of its own ([`Input::Predicted`]).
    pub(super) fn check_overlaps(&mut self, channel: &str) {
        self.predict_remote(channel);
        self.refresh_pairs(channel);
        let mine = self.mine(channel);
        let alerts: Vec<(Response, bool)> = self
            .presence
            .new_overlaps(channel, &mine, now())
            .into_iter()
            .filter_map(|mut overlap| {
                self.told
                    .remote(&mut overlap, self.clock.now())
                    .then_some((Response::Overlap(overlap), false))
            })
            .collect();
        self.tell_overlaps(channel, alerts);
        let local: Vec<(Worktree, BTreeSet<String>)> = self
            .local(channel)
            .into_iter()
            .map(|(worktree, _, paths)| (worktree, paths))
            .collect();
        let local: Vec<(Worktree, &BTreeSet<String>)> = local
            .iter()
            .map(|(worktree, paths)| (worktree.clone(), paths))
            .collect();
        let mut overlaps = self.presence.new_local_overlaps(channel, &local, now());
        if overlaps.is_empty() {
            return;
        }
        let channel = channel.to_owned();
        let inputs = self.inputs.clone();
        self.checking += 1;
        tokio::spawn(async move {
            for overlap in &mut overlaps {
                predict(overlap).await;
            }
            let _ = inputs.send(Input::Predicted(channel, overlaps));
        });
    }

    /// Tells watching clients about the `overlaps` between this machine's own worktrees in
    /// `channel`, which Git has said the shared lines of ([`Daemon::check_overlaps`]).
    pub(super) fn predicted(&mut self, channel: &str, overlaps: Vec<LocalOverlap>) {
        self.checking = self.checking.saturating_sub(1);
        let alerts = overlaps
            .into_iter()
            .filter_map(|mut overlap| {
                self.predictions.note(&overlap);
                let escalated = self.told.local_news(&mut overlap, self.clock.now())?;
                Some((Response::LocalOverlap(overlap), escalated))
            })
            .collect();
        self.tell_overlaps(channel, alerts);
    }

    /// Tells watching clients each of `alerts`, an overlap in `channel` and whether it turns
    /// changes told as apart into a clash.
    fn tell_overlaps(&mut self, channel: &str, alerts: Vec<(Response, bool)>) {
        for (alert, escalated) in alerts {
            tracing::info!("overlap in {channel}: {alert:?}");
            self.tell_watchers(&alert);
            let kind = overlap_kind(&alert);
            let woke = !matches!(&alert, Response::LocalOverlap(overlap) if overlap.quiet());
            self.noticed(&alert, Some(kind), woke, escalated);
            self.count(&Usage::OverlapNotified(usage::OverlapNotified {
                local: matches!(alert, Response::LocalOverlap(_)),
                kind,
                woke,
                escalated,
            }));
        }
    }

    /// Counts `event` in analytics, as one of this daemon's organisation's.
    pub(super) fn count(&self, event: &Usage) {
        zirc_core::telemetry::record_in(event, Some(&self.org.name));
    }
}

/// What `alert`, an overlap, amounts to, as its telemetry says: the worst of its paths.
fn overlap_kind(alert: &Response) -> usage::OverlapKind {
    use usage::OverlapKind::{Apart, Path, SameLines};
    match alert {
        Response::LocalOverlap(overlap) if !overlap.same_lines.is_empty() => SameLines,
        Response::LocalOverlap(overlap) if overlap.quiet() => Apart,
        Response::LocalOverlap(overlap) => kind_of_unknown(
            overlap
                .paths
                .iter()
                .filter(|path| !overlap.apart.contains(path)),
        ),
        Response::Overlap(overlap) => kind_of_unknown(overlap.paths.iter()),
        _ => Path,
    }
}

/// [`usage::OverlapKind::Lockfile`] when every one of `paths` is a lockfile, else
/// [`usage::OverlapKind::Path`].
fn kind_of_unknown<'a>(mut paths: impl Iterator<Item = &'a String>) -> usage::OverlapKind {
    if paths.all(|path| zirc_core::hunks::is_lockfile(path)) {
        usage::OverlapKind::Lockfile
    } else {
        usage::OverlapKind::Path
    }
}
