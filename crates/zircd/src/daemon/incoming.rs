//! Lines from the IRC server: messages, joins, tags and the rest, as they arrive.

use zirc_core::ipc::Response;
use zirc_irc::Line;
use zirc_proto::Event;

use super::{Daemon, heard, names_us_as_op, now, vouch};

impl Daemon {
    /// A line from the server. A working set sent as a TAGMSG, out of history, is read as
    /// the PRIVMSG it stands for: heard once by its id, our own echo told apart, overlaps
    /// checked. One in a plain PRIVMSG, the form before 0.1.24 that history still holds,
    /// is left out.
    pub(super) fn receive(&mut self, line: &Line) {
        // The newest message heard in a channel, which what's missed after it is asked from.
        if matches!(line.command.as_str(), "PRIVMSG" | "TAGMSG")
            && let Some(channel) = line.params.first().filter(|target| target.starts_with('#'))
        {
            self.heard_in(channel, line);
        }
        if self.ops.is_some() && matches!(line.command.as_str(), "PRIVMSG" | "REDACT") {
            let delivery = line.params.first().and_then(|channel| heard(channel, line));
            let delivery = delivery.map(|(mut delivery, _)| {
                vouch(&mut delivery, self.zirc_dev);
                delivery
            });
            self.ops_heard(line, delivery.as_ref());
        }
        // Machines' lists of their live sessions are for apps and for whether a state's
        // owner is still on it ([`live::Elsewhere`]): no watch needs them.
        if let Some(live) = line
            .tag(zirc_proto::EVENT_TAG)
            .and_then(|payload| zirc_proto::from_payload(payload).ok())
            .and_then(|message| zirc_proto::session::Live::of(&message.event))
        {
            if let Some(machine) = line
                .source
                .as_deref()
                .and_then(|source| source.split('!').next())
            {
                self.elsewhere.said(machine, &live.sessions);
            }
            return;
        }
        match zirc_core::presence::as_privmsg(line) {
            Some(privmsg) => self.received(&privmsg),
            None if zirc_core::presence::old_working(line) => {}
            None => self.received(line),
        }
    }

    /// A direct message to this machine: the owner's `resume`, a query or its answer, or the
    /// owner's line for the agents, told to the watchers.
    fn heard_direct(&mut self, line: &Line, from: &str, body: &str) {
        let account = line.tag("account");
        let sender = zirc_core::agents::Sender::of(account, line.source.as_deref());
        if self.resume_on_owners_word(account, from, body) {
            return;
        }
        match self
            .queried_or_answered(sender, from, body)
            .map(Ok)
            .or_else(|| self.direct(sender, from, body))
        {
            Some(Ok(mut direct)) => {
                self.remember_direct(&mut direct, line);
                self.tell_watchers(&Response::Direct(direct));
            }
            // The owner named a session that isn't here: say so, rather than drop it.
            Some(Err(notice)) => {
                if let Some(client) = &self.client {
                    let _ = client.send(Line::new("NOTICE", [from, notice.as_str()]));
                }
            }
            None => {}
        }
    }

    /// Whose machine `from`, who sent `line`, is, as its host says: a CHGHOST's new one, or
    /// the line's own. This machine's own host says whose it is, on zirc.dev's servers.
    fn heard_host(&mut self, line: &Line, from: &str) {
        let host = if line.command == "CHGHOST" {
            line.params
                .get(1)
                .and_then(|host| zirc_proto::names::owner_from_host(host))
        } else {
            zirc_core::agents::host_owner(line.source.as_deref())
        };
        match host {
            Some(owner) => {
                self.machines
                    .insert(from.to_ascii_lowercase(), owner.to_owned());
            }
            None => {
                self.machines.remove(&from.to_ascii_lowercase());
            }
        }
        // This machine's own host says whose it is, on zirc.dev's servers: its owner, then,
        // as when they change their handle, which renames none of their machines.
        if self.zirc_dev
            && let Some(owner) = host
            && self
                .client
                .as_ref()
                .is_some_and(|client| client.nick().eq_ignore_ascii_case(from))
            && !self
                .owner
                .as_deref()
                .is_some_and(|o| o.eq_ignore_ascii_case(owner))
        {
            tracing::info!(owner, "this machine's host names its owner");
            self.owner = Some(owner.to_owned());
        }
    }

    pub(super) fn received(&mut self, line: &Line) {
        let from = line
            .source
            .as_deref()
            .and_then(|source| source.split('!').next())
            .unwrap_or_default();
        match line.tag("account") {
            Some(account) if !account.is_empty() && account != "*" => {
                self.accounts
                    .insert(from.to_ascii_lowercase(), account.to_owned());
            }
            _ => {
                self.accounts.remove(&from.to_ascii_lowercase());
            }
        }
        self.heard_host(line, from);
        if self.client.is_none() {
            return;
        }
        let Some(client) = &self.client else { return };
        let ours = from == client.nick();
        let me = client.nick().to_owned();
        let param = |i: usize| line.params.get(i).map(String::as_str);
        // Messages and reactions can come again, as the server sends one twice, or history
        // replays one heard live: one already heard isn't news again.
        if matches!(line.command.as_str(), "PRIVMSG" | "TAGMSG")
            && let Some(id) = line.tag("msgid")
            && !self.heard.first_time(id)
        {
            return;
        }
        match (line.command.as_str(), param(0), param(1)) {
            ("JOIN", Some(channel), _) => self.someone_joined(channel, ours),
            ("PART", Some(channel), _) if ours => {
                self.joined.remove(channel);
            }
            ("PART", Some(channel), _) => self.presence.left(channel, from),
            ("KICK", Some(channel), Some(nick)) if nick == me => {
                self.joined.remove(channel);
            }
            ("KICK", Some(channel), Some(nick)) => self.presence.left(channel, nick),
            // Why the server didn't let us join a channel: tried again in a while.
            ("403" | "405" | "471" | "473" | "474" | "475" | "477" | "489", _, Some(channel)) => {
                let why = line.params.last().map_or("", String::as_str);
                tracing::warn!("couldn't join {channel}: {why}");
            }
            // Joining a channel nobody's in makes us its maker, and Ergo gives a maker ops. A
            // machine shouldn't have them: the organisation's owners get them instead.
            // A guest's terminal can't, nor needs to: the gateway lets it say nothing else.
            ("353", _, _) if self.org.guest.is_none() && names_us_as_op(&line.params, &me) => {
                if let Some(channel) = param(2) {
                    let _ = client.send(Line::new("MODE", [channel, "-o", me.as_str()]));
                }
            }
            ("QUIT", _, _) => self.presence.quit(from),
            ("NICK", Some(new), _) if !ours => self.presence.renamed(from, new),
            ("PRIVMSG", Some(target), Some(body)) if target == me && !ours => {
                self.heard_direct(line, from, body);
            }
            // This machine's own posts were told and kept as they were sent: one coming back,
            // as history replayed or from another client of its account, isn't another
            // machine's work, nor news.
            ("PRIVMSG", Some(channel), Some(body)) if channel.starts_with('#') && ours => {
                self.echoed(channel, from, body, line);
            }
            // An app joined, or attached to an account that's in the channel: say again what
            // we're working on, as for someone joining.
            ("TAGMSG", Some(channel), _) if !ours && zirc_core::presence::asks(line) => {
                self.someone_joined(channel, false);
            }
            ("TAGMSG", Some(target), _) if !ours => self.reacted(target, from, line),
            ("PRIVMSG", Some(channel), Some(_)) if channel.starts_with('#') => {
                let Some((mut delivery, zirc)) = heard(channel, line) else {
                    return;
                };
                vouch(&mut delivery, self.zirc_dev);
                self.remember(&delivery);
                if !zirc {
                    self.tell_watchers(&Response::Delivery(delivery));
                    return;
                }
                self.lease_heard(channel, from, &delivery.message.event);
                // An agent room's lines count as its repository's.
                let channel = self.home_channel(channel);
                let default = self.default_branch(&channel);
                self.presence
                    .apply(&channel, from, &delivery.message, now(), default.as_deref());
                if matches!(delivery.message.event, Event::Working { .. }) {
                    self.check_overlaps(&channel);
                }
                let push = delivery.message.event.clone();
                self.tell_watchers(&Response::Delivery(delivery));
                self.check_push(&channel, from, &push);
            }
            _ => {}
        }
    }
}
