//! Everything zirc knows about someone, for the card an app shows when they're clicked or
//! hovered: who they are, whose machine it is or which machines are theirs, whether they're
//! away, which channels we share, what they're changing and have said they'll do, what they
//! last said, and, once asked with WHOIS, their account and how long they've been idle.

use std::collections::BTreeSet;

use chrono::{DateTime, NaiveDateTime, Utc};
use serde::Serialize;
use ts_rs::TS;
use zirc_line::Line;

use super::Chat;
use super::conversation::{EntryKind, Kind, fold};
use crate::presence::{Claim, Intent, Member};

/// What a WHOIS said about someone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Whois {
    pub(super) account: Option<String>,
    pub(super) idle_since: Option<DateTime<Utc>>,
    pub(super) signed_on: Option<DateTime<Utc>>,
}

/// Someone, as far as zirc knows: [`Chat::profile`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Their nick.
    pub nick: String,
    /// Their IRC real name: a machine's is its owner.
    pub realname: Option<String>,
    /// The name they show as, [`Chat::shown_name`]: the card's heading in every app.
    pub name: String,
    /// For a machine, whose it is and its own name: `("mattias", "mbp")`.
    pub machine: Option<(String, String)>,
    /// For a person, the nicks of their machines that are around, sorted.
    pub machines: Vec<String>,
    /// Why they're away, when they are; see [`Chat::away`].
    pub away: Option<String>,
    /// The channels we're both in, sorted.
    pub channels: Vec<String>,
    /// What they're changing, and for a person what their machines are, by channel.
    pub working: Vec<(String, Member)>,
    /// What they, or their machines' sessions, said they're about to change, by channel.
    pub intents: Vec<(String, Intent)>,
    /// What they, or their machines' sessions, said they've taken on, by channel.
    pub claims: Vec<(String, Claim)>,
    /// Where they last said something we saw, by its title ([`Chat::title`]), or `None` in
    /// the conversation we're in, and when.
    pub last_said: Option<(Option<String>, DateTime<Utc>)>,
    /// Their account, once a WHOIS has said.
    pub account: Option<String>,
    /// Since when they've been idle, once a WHOIS has said.
    pub idle_since: Option<DateTime<Utc>>,
    /// When they connected, once a WHOIS has said.
    pub signed_on: Option<DateTime<Utc>>,
    /// When they were last active, while they're away or offline, if we saw them go
    /// ([`Chat::active_at`]).
    pub active_at: Option<DateTime<Utc>>,
}

/// Whether someone is around, as a profile card says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// In a channel with us and not away.
    Here,
    /// Connected but away, saying why, which may be empty: set by hand, or by an app that
    /// has seen no input for a while.
    Away(String),
    /// None of their apps is connected. Ergo keeps their account in its channels and marks
    /// it away with [`OFFLINE_AWAY`]; that is not the same promise as away.
    Offline,
    /// Neither known: not in any channel with us.
    Unknown,
}

/// The away message Ergo gives an always-on account while none of its clients is connected.
/// Ergo's own text, so it is how offline is told from away.
pub const OFFLINE_AWAY: &str = "User is currently disconnected";

/// How long without input on a computer, in seconds, before its app goes away by itself.
pub const IDLE_AFTER_SECS: u64 = 600;

/// The away message a bare `/away` sends, as IRC needs one: it gives no reason, and every app
/// says it as `away` alone ([`away_reason`]), never `away: away`.
pub const BARE_AWAY: &str = "away";

/// Why someone's away, from their away message: none for [`BARE_AWAY`].
#[must_use]
pub fn away_reason(why: &str) -> &str {
    if why == BARE_AWAY { "" } else { why }
}

/// The away message an app gives itself when idle or locked ([`Chat::set_idle`]); said
/// `away: idle`.
pub const IDLE_AWAY: &str = "idle";

/// The away message an app gives itself when the screen is locked: last active then.
pub const LOCKED_AWAY: &str = "locked";

/// The away message a phone's app gives itself in the background: last active then.
pub const BACKGROUND_AWAY: &str = "in the background";

/// Someone's dot, green, amber or grey, as every app draws it beside their avatar in
/// lists, headers and profiles: [`Standing`] without why they're away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum Presence {
    /// Here and not away.
    Here,
    /// Away.
    Away,
    /// Not connected, or not known.
    Gone,
}

impl Standing {
    /// Someone's standing, `here` with us (in a conversation, or a channel we share) and
    /// `away` saying why when they are: gone when not here, else away, else here. Every
    /// list's, header's and profile's is this one.
    #[must_use]
    pub fn of(here: bool, away: Option<&str>) -> Self {
        match away {
            _ if !here => Self::Unknown,
            Some(OFFLINE_AWAY) => Self::Offline,
            Some(why) => Self::Away(away_reason(why).to_owned()),
            None => Self::Here,
        }
    }

    /// Its dot: [`Presence`].
    #[must_use]
    pub fn presence(&self) -> Presence {
        match self {
            Self::Here => Presence::Here,
            Self::Away(_) => Presence::Away,
            Self::Offline | Self::Unknown => Presence::Gone,
        }
    }

    /// As shown after their name: `here`, `away`, `away: lunch`, `offline`, or nothing.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Here => "here".to_owned(),
            Self::Away(why) if why.is_empty() => "away".to_owned(),
            Self::Away(why) => format!("away: {why}"),
            Self::Offline => "offline".to_owned(),
            Self::Unknown => String::new(),
        }
    }
}

/// How someone's away is said after their name, in a list or the people panel: `away`,
/// `away: lunch`, or `offline` for the server's own message, from their away message `why`,
/// which may be empty ([`Standing::label`]).
#[must_use]
pub fn away_label(why: &str) -> String {
    Standing::of(true, Some(why)).label()
}

/// The same on a line of its own, as a conversation's header or our own status says it:
/// `Away`, `Away: lunch`.
#[must_use]
pub fn away_title(why: &str) -> String {
    let label = away_label(why);
    let mut chars = label.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// What a profile card says, the same in every app, which only draws it: [`Profile::card`].
/// Owned and flat, so web serialises it and the phones' bindings carry it as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct Card {
    /// Their name as they show it, `Ada Lovelace`, or a machine's `mattias · mbp`.
    pub title: String,
    /// Their nick, which every app sets in mono under the name, as Discord's card has it:
    /// none when the name is it, and on a machine's or an agent session's card.
    pub handle: Option<String>,
    /// Whether they're around, in words: `here`, `away: lunch`, `offline`, or nothing when
    /// unknown ([`Standing::label`]); on an agent session's card, whose away it is, as a
    /// session is never away itself: `Maya Chen is away: lunch`.
    pub standing: String,
    /// Whether they're away.
    pub away: bool,
    /// Their dot ([`Standing::presence`]).
    pub presence: Presence,
    /// For a machine, whose agent it is and where, the same in every app: `Agent of dana,
    /// on matt`, and with [`Card::with_session`] `Agent of dana, on matt · cc-55aa66`. The
    /// machine and session are said here, never in a message's header.
    pub agent: Option<String>,
    /// A line each under the name and the handle: their real name, their account, how long
    /// they've been idle and on, the channels we share, and when they last spoke. Their
    /// machines are a section of their own ([`super::Chat::card_machines`]).
    pub about: Vec<String>,
    /// What they're changing, about to change and have taken on, each under a heading, for
    /// each that has any.
    pub work: Vec<CardWork>,
    /// For an agent session's card, its machine and id, which apps set in mono under the
    /// title: `mbp · cc-4e21`.
    pub ids: Option<String>,
    /// For an agent session's card, what it's taken on and for how long, as the Active now
    /// strip says it: `fix the coupon field's layout shift · 4m`.
    pub doing: Option<String>,
}

/// A heading of a profile card's work and its lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CardWork {
    /// `changing`, `about to change` or `taken on`.
    pub heading: String,
    /// What's under it.
    pub lines: Vec<String>,
}

impl Profile {
    /// What a card about them says at `now`, giving times on the app's clock, `wall` saying
    /// what a moment is in its time zone, as [`card_time`] writes them.
    #[must_use]
    pub fn card(&self, now: DateTime<Utc>, wall: impl Fn(DateTime<Utc>) -> NaiveDateTime) -> Card {
        let when = |at: DateTime<Utc>| card_time(wall(at), wall(now));
        let title = match &self.machine {
            Some((owner, machine)) => super::machine_label(owner, machine),
            None => self.name.clone(),
        };
        let standing = Standing::of(!self.channels.is_empty(), self.away.as_deref());

        // By the name they show, their nick under it, as Discord's card has it.
        let handle = (self.machine.is_none() && !self.name.eq_ignore_ascii_case(&self.nick))
            .then(|| self.nick.clone());
        let mut about = Vec::new();
        if self.machine.is_none()
            && let Some(realname) = self.realname.as_deref().map(str::trim).filter(|name| {
                !name.eq_ignore_ascii_case(&self.nick) && !name.eq_ignore_ascii_case(&self.name)
            })
        {
            about.push(realname.to_owned());
        }
        let mut whois = Vec::new();
        if let Some(account) = &self.account {
            whois.push(format!("account {account}"));
        }
        // Last active, for those not here, from when we saw them go.
        if let Some(at) = self.active_at.filter(|_| standing != Standing::Here) {
            whois.push(format!("active {}", when(at)));
        }
        if let Some(since) = self.idle_since {
            whois.push(format!("idle {}", span(seconds(now - since))));
        }
        if let Some(on) = self.signed_on {
            whois.push(format!("on since {}", when(on)));
        }
        if !whois.is_empty() {
            about.push(whois.join(" · "));
        }
        if !self.channels.is_empty() {
            about.push(format!("in {}", self.channels.join(", ")));
        }
        match &self.last_said {
            Some((Some(place), at)) => about.push(format!("last said {} in {place}", when(*at))),
            Some((None, at)) => about.push(format!("last said {} here", when(*at))),
            None => {}
        }

        Card {
            title,
            handle,
            standing: standing.label(),
            away: matches!(standing, Standing::Away(_)),
            presence: standing.presence(),
            agent: self.agent(),
            about,
            work: self.work(),
            ids: None,
            doing: None,
        }
    }

    /// What they're changing, about to change and have taken on: a heading and its lines,
    /// for each that has any.
    fn work(&self) -> Vec<CardWork> {
        let mut work = Vec::new();
        let changing: Vec<String> = self
            .working
            .iter()
            .map(|(channel, member)| {
                let place = member
                    .branch
                    .as_ref()
                    .map(|branch| format!(" [{branch}]"))
                    .unwrap_or_default();
                let session = member
                    .session
                    .as_ref()
                    .map(|session| format!(" {session}"))
                    .unwrap_or_default();
                // How many files, never which: a card is too narrow for their paths.
                format!(
                    "{channel} {}{place}{session}: {}",
                    member.nick,
                    super::describe::files(member.paths.len())
                )
            })
            .collect();
        let intents: Vec<String> = self
            .intents
            .iter()
            .map(|(channel, intent)| {
                let who = intent.nick.clone();
                format!("{channel} {who}/{}: {}", intent.session, intent.summary)
            })
            .collect();
        let claims: Vec<String> = self
            .claims
            .iter()
            .map(|(channel, claim)| {
                let who = claim.nick.clone();
                format!("{channel} {who}/{}: {}", claim.session, claim.task)
            })
            .collect();
        for (heading, lines) in [
            ("changing", changing),
            ("about to change", intents),
            ("taken on", claims),
        ] {
            if !lines.is_empty() {
                work.push(CardWork {
                    heading: heading.to_owned(),
                    lines,
                });
            }
        }
        work
    }
}

impl Profile {
    /// The card for the agent `session` on this machine, as opened from its row or from
    /// something it said: only what that session is changing, about to change and has
    /// taken on, and `Agent of dana, on matt · cc-55aa66`.
    #[must_use]
    pub fn session_card(
        &self,
        session: &str,
        now: DateTime<Utc>,
        wall: impl Fn(DateTime<Utc>) -> NaiveDateTime,
    ) -> Card {
        let ours = |s: &str| s.eq_ignore_ascii_case(session);
        let mut narrowed = self.clone();
        narrowed
            .working
            .retain(|(_, member)| member.session.as_deref().is_some_and(ours));
        narrowed.intents.retain(|(_, intent)| ours(&intent.session));
        narrowed.claims.retain(|(_, claim)| ours(&claim.session));
        narrowed.card(now, wall).with_session(session)
    }

    /// For a machine, whose agent it is and where: `Agent of dana, on matt`.
    fn agent(&self) -> Option<String> {
        let (owner, machine) = self.machine.as_ref()?;
        Some(format!("Agent of {owner}, on {machine}"))
    }
}

impl Card {
    /// The card opened from what one of the machine's sessions said, naming that session.
    #[must_use]
    pub fn with_session(mut self, session: &str) -> Self {
        if let Some(agent) = &mut self.agent
            && !session.is_empty()
        {
            agent.push_str(" · ");
            agent.push_str(session);
        }
        self
    }
}

/// The time of day `at`, a wall time on an app's clock, as every app writes it: `15:48`. A
/// message's time, and the clock in every other time zirc shows.
#[must_use]
pub fn clock(at: NaiveDateTime) -> String {
    at.format("%H:%M").to_string()
}

/// When `at` was, on an app's clock, as a profile card says it: `11:50` on the day of
/// `now`, `Thu 11:50` before. Both are wall times in the app's time zone.
#[must_use]
pub fn card_time(at: NaiveDateTime, now: NaiveDateTime) -> String {
    if at.date() == now.date() {
        clock(at)
    } else {
        format!("{} {}", at.format("%a"), clock(at))
    }
}

/// How long `seconds` is, roughly, in its largest whole unit: `40s`, `5m`, `3h`, `2d`. Every
/// span zirc shows is this one.
#[must_use]
pub fn span(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// When `at` was, from `now`, as a list's right edge says it: `now`, `5m`, `3h`, `9d`,
/// `1 mo`, `1 yr`. Every list's time is this one.
#[must_use]
pub fn ago_short(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let seconds = seconds(now - at);
    let days = seconds / 86_400;
    match seconds {
        0..60 => "now".to_owned(),
        60..86_400 => span(seconds),
        _ if days < 30 => format!("{days}d"),
        _ if days < 365 => format!("{} mo", days / 30),
        _ => format!("{} yr", days / 365),
    }
}

/// How long ago something was, `seconds` back: `just now`, `12m ago`, `3h ago`, `2d ago`.
#[must_use]
pub fn ago(seconds: u64) -> String {
    if seconds < 60 {
        "just now".to_owned()
    } else {
        format!("{} ago", span(seconds))
    }
}

/// How many whole seconds `span` is, none when it's negative.
fn seconds(span: chrono::Duration) -> u64 {
    u64::try_from(span.num_seconds()).unwrap_or_default()
}

/// What some people and their machines are at ([`Chat::work_of`]), each with the channel it
/// counts as.
#[derive(Default)]
struct Work {
    /// Who has uncommitted changes.
    working: Vec<(String, Member)>,
    /// What they're about to change.
    intents: Vec<(String, Intent)>,
    /// What they've taken on.
    claims: Vec<(String, Claim)>,
}

impl Chat {
    /// When `nick` was last active, while they're away or offline and we saw them go: the
    /// time of their `AWAY` line, ten minutes before it for `idle` (which the app says after
    /// that long without input; [`IDLE_AFTER_SECS`]).
    #[must_use]
    pub fn active_at(&self, nick: &str) -> Option<DateTime<Utc>> {
        self.away(nick)?;
        self.last_active.get(&fold(nick)).copied()
    }

    /// `nick`'s standing: here while they're in a channel we share, away as the server
    /// says ([`Chat::away`]). Their profile's, and their direct-message row's dot.
    #[must_use]
    pub fn standing(&self, nick: &str) -> Standing {
        let here = self
            .conversations
            .iter()
            .any(|c| c.kind == Kind::Channel && c.has_member(nick));
        Standing::of(here, self.away(nick))
    }

    /// What those `theirs` takes in are changing, about to change at `seconds` and have
    /// taken on, in every channel, each by the channel it counts as ([`super::home_channel`]):
    /// a repository's for what's said in its agents' room.
    fn work_of(&self, theirs: &dyn Fn(&str) -> bool, seconds: u64) -> Work {
        let mut work = Work::default();
        for conversation in self
            .conversations
            .iter()
            .filter(|c| c.kind == Kind::Channel)
        {
            let name = &conversation.name;
            let home = super::home_channel(name).to_owned();
            work.working.extend(
                self.presence
                    .members(name)
                    .into_iter()
                    .filter(|member| theirs(&member.nick))
                    .map(|member| (home.clone(), member)),
            );
            work.intents.extend(
                self.presence
                    .intents(name, seconds)
                    .into_iter()
                    .filter(|intent| theirs(&intent.nick))
                    .map(|intent| (home.clone(), intent)),
            );
            work.claims.extend(
                self.presence
                    .claims(name)
                    .into_iter()
                    .filter(|claim| theirs(&claim.nick))
                    .map(|claim| (home.clone(), claim)),
            );
        }
        work
    }

    /// Everything known about `nick`. [`Chat::whois`] asks the server for the rest.
    #[must_use]
    pub fn profile(&self, nick: &str) -> Profile {
        let realname = self.realname(nick).map(str::to_owned);
        let everyone: BTreeSet<String> = self
            .conversations
            .iter()
            .flat_map(|conversation| conversation.members.values())
            .map(|member| member.nick.clone())
            .collect();
        // Whose machine it is, or whose machines a person's are, as their hosts say.
        let whose = self.machine(nick);
        let machines: Vec<String> = if whose.is_some() {
            Vec::new()
        } else {
            everyone
                .iter()
                .filter(|other| {
                    self.machine_owner(other)
                        .is_some_and(|owner| owner.eq_ignore_ascii_case(nick))
                })
                .cloned()
                .collect()
        };
        let theirs = |other: &str| {
            other.eq_ignore_ascii_case(nick)
                || machines.iter().any(|m| m.eq_ignore_ascii_case(other))
        };

        let now = (self.clock)();
        let seconds = u64::try_from(now.timestamp()).unwrap_or_default();
        let mut channels = Vec::new();
        let Work {
            working,
            intents,
            claims,
        } = self.work_of(&theirs, seconds);
        let mut last_said: Option<(&str, DateTime<Utc>)> = None;
        for conversation in &self.conversations {
            let name = &conversation.name;
            // A repository's agents' room is the repository's.
            let home = super::home_channel(name);
            if conversation.kind == Kind::Channel
                && conversation.has_member(nick)
                && !channels
                    .iter()
                    .any(|c: &String| c.eq_ignore_ascii_case(home))
            {
                channels.push(home.to_owned());
            }
            let said = conversation.entries.iter().rev().find(|entry| {
                entry.from.eq_ignore_ascii_case(nick)
                    && matches!(
                        entry.kind,
                        EntryKind::Message(_) | EntryKind::Action(_) | EntryKind::Notice(_)
                    )
            });
            if let Some(entry) = said
                && last_said
                    .as_ref()
                    .is_none_or(|(_, time)| entry.time > *time)
            {
                last_said = Some((name, entry.time));
            }
        }
        channels.sort_by_key(|name| fold(name));

        let whois = self.whois.get(&fold(nick)).cloned().unwrap_or_default();
        Profile {
            nick: nick.to_owned(),
            realname,
            name: self.shown_name(nick),
            machine: whose,
            machines,
            away: self.away(nick).map(str::to_owned),
            channels,
            working,
            intents,
            claims,
            last_said: last_said.map(|(name, at)| {
                let here = name.eq_ignore_ascii_case(&self.selected().name);
                ((!here).then(|| self.title(name)), at)
            }),
            account: whois.account,
            idle_since: whois.idle_since,
            signed_on: whois.signed_on,
            active_at: self.active_at(nick),
        }
    }

    /// A WHOIS answer: `RPL_WHOISUSER` (nick, user, host, `*`, real name), `RPL_WHOISACCOUNT`,
    /// or `RPL_WHOISIDLE` (seconds idle, when they connected).
    pub(super) fn whois_reply(&mut self, line: &Line) {
        let param = |i: usize| line.params.get(i).map_or("", String::as_str);
        let nick = fold(param(1));
        match line.command.as_str() {
            "311" => {
                self.realnames.insert(nick, param(5).to_owned());
            }
            "330" => {
                self.whois.entry(nick).or_default().account =
                    Some(param(2).to_owned()).filter(|account| !account.is_empty());
            }
            "317" => {
                let now = (self.clock)();
                let whois = self.whois.entry(nick).or_default();
                whois.idle_since = param(2)
                    .parse()
                    .ok()
                    .map(|idle: i64| now - chrono::Duration::seconds(idle));
                whois.signed_on = param(3)
                    .parse()
                    .ok()
                    .and_then(|at: i64| DateTime::from_timestamp(at, 0));
            }
            _ => {}
        }
    }

    /// Asks the server about `nick` (WHOIS): their account, how long they've been idle and
    /// when they connected, for [`Chat::profile`]. Returns what to send.
    #[must_use]
    pub fn whois(&self, nick: &str) -> Vec<Line> {
        // The Server channel's name is no one's: asked, the server says there's no such nick.
        if self.is_connected() && !self.is_server(nick) {
            vec![Line::new("WHOIS", [nick])]
        } else {
            Vec::new()
        }
    }
}

impl Chat {
    /// The card of `nick`'s agent session `session`, as opened from its row or its header:
    /// titled by its name and its owner's, `checkout · Maya Chen`, its machine and id under
    /// that, `mbp · cc-4e21`, what it's doing, `fix the coupon field · 4m`, a line for whose
    /// agent it is and its tool, `Maya Chen's agent · Claude Code`, then what it's changing
    /// and has taken on ([`Profile::session_card`]). Its standing is its owner's.
    #[must_use]
    pub fn session_card(
        &self,
        nick: &str,
        session: &str,
        now: DateTime<Utc>,
        wall: impl Fn(DateTime<Utc>) -> NaiveDateTime,
    ) -> Card {
        let profile = self.profile(nick);
        let mut card = profile.session_card(session, now, wall);
        let label = self.session_label(session);
        if let Some((owner, machine)) = &profile.machine {
            let owner = self.shown_name(owner);
            card.title = format!("{label} · {owner}");
            card.ids = Some(format!("{machine} · {session}"));
            // Whose agent, as every row says it: the one place that's composed.
            let (whose, _) = super::agent_name(None, &owner);
            let mut line = vec![whose];
            if let Some(tool) = self.session_tools.get(session) {
                line.push(tool_name(tool));
            }
            card.agent = Some(line.join(" · "));
            // A session is never away itself: its owner's away is what the card says.
            if card.away {
                card.standing = format!("{owner} is {}", card.standing);
            }
        } else {
            card.title = label;
        }
        card.doing = self
            .claim(nick, session)
            .map(|(task, since)| format!("{task} · {}", ago_short(since, now)));
        card
    }
}

/// A tool's name as people read it: `Claude Code` for `claude-code`.
pub(super) fn tool_name(tool: &str) -> String {
    match tool {
        zirc_proto::CLAUDE_CODE => "Claude Code".to_owned(),
        zirc_proto::CODEX => "Codex".to_owned(),
        other => other.to_owned(),
    }
}
