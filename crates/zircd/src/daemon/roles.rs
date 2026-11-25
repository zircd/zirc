//! A role's seat taken and left by this machine's sessions ([`zirc_proto::roles`]), and what
//! comes to the holder from the role's nick.
//!
//! The seat is the lease [`roles::lease`], agreed as every lease is ([`zirc_core::leases`]):
//! taken only when no session holds it, held as long as its session's watch lives (zircd
//! asks for more before it ends, and never gives it back for running nothing), and given back
//! when the session leaves the role or its watch has been gone a while, as any lease is. A
//! session that asked at the same moment as another and came second is told so, and its
//! request withdrawn: a role never queues.
//!
//! The role's nick, which the server's sidecar keeps, wakes the holder with what came to the
//! role ([`roles::Wake`]): taken in only from that nick's own account, for a session that
//! holds the role here, so no one else can speak for a role. What the holder says as the
//! role goes to the role's nick ([`roles::Say`]), whose sidecar says it if the session holds
//! the seat.

use anyhow::{Context, Result, bail};
use zirc_core::ipc::{Direct, Response};
use zirc_core::leases::{self, LeaseEvent};
use zirc_proto::Message;
use zirc_proto::roles::{self, Say, Wake};

use super::Daemon;
use crate::clock::unix_ms as now_ms;

/// How long before its turn ends a role's lease is asked for again, in milliseconds: well
/// before, so a busy machine never lets it lapse.
const RENEW_BEFORE: u64 = leases::MAX_SECONDS * 1000 / 2;

impl Daemon {
    /// The role `session` holds here, if it holds one.
    pub(super) fn role_of(&self, session: &str) -> Option<String> {
        self.roles_held()
            .into_iter()
            .find_map(|(holder, role)| (holder == session).then_some(role))
    }

    /// Each role a session here holds now, with the session.
    pub(super) fn roles_held(&self) -> Vec<(String, String)> {
        let Some(client) = &self.client else {
            return Vec::new();
        };
        let now = now_ms();
        self.leases
            .leases
            .mine(client.nick(), now)
            .into_iter()
            .filter(|turn| turn.start <= now)
            .filter_map(|turn| {
                let role = roles::of_lease(&turn.want.name)?;
                Some((turn.want.session.clone(), role.to_owned()))
            })
            .collect()
    }

    /// [`zirc_core::ipc::Request::RoleTake`].
    pub(super) async fn role_take(
        &mut self,
        role: &str,
        session: &str,
        repo: Option<&str>,
    ) -> Result<Response> {
        if let Some(held) = self.role_of(session)
            && held != role
        {
            bail!(
                "{session} is the {held} already: a session holds one role, so leave it first \
                 ({} role leave)",
                zirc_proto::PROGRAM
            );
        }
        let server = self
            .content_server()
            .context("this machine can't ask its organisation's server about roles")?;
        let standing = server.roles().await.map_err(anyhow::Error::msg)?;
        let Some((definition, seat)) = standing.role(role) else {
            let names: Vec<&str> = standing
                .listing
                .roles
                .iter()
                .map(|role| role.name.as_str())
                .collect();
            bail!(
                "there's no {role} role: {}",
                if names.is_empty() {
                    "your organisation's admins add roles on zirc.dev".to_owned()
                } else {
                    format!("its roles are {}", names.join(", "))
                }
            );
        };
        if repo != Some(definition.repository.as_str()) {
            bail!(
                "the {role} is taken in a checkout of {}: run this there",
                definition.repository
            );
        }
        let me = self.lease_nick()?;
        let now = now_ms();
        let name = roles::lease(role);
        let lease = self.leases.leases.lease(&name, now, &me);
        if let Some(holder) = lease
            .holders
            .iter()
            .chain(&lease.waiting)
            .find(|holder| !(holder.mine && holder.session == session))
        {
            bail!(
                "{}/{} is the {role}: ask it with {} ask --to {}/{} \"…\", or wait until it \
                 leaves the role",
                holder.nick,
                holder.session,
                zirc_proto::PROGRAM,
                holder.nick,
                holder.session
            );
        }
        if lease.holders.is_empty() && lease.waiting.is_empty() {
            self.want(&me, &name, session, leases::MAX_SECONDS, now);
        }
        let seat = seat.cloned().unwrap_or_else(|| roles::Seat {
            role: role.to_owned(),
            repository: definition.repository.clone(),
            holder: None,
            last: None,
            waiting: 0,
            wakes_on: definition.wakes_on(),
            woke_today: 0,
        });
        Ok(Response::RoleTaken {
            role: definition.clone(),
            seat,
        })
    }

    /// [`zirc_core::ipc::Request::RoleLeave`].
    pub(super) fn role_leave(&mut self, session: &str) -> Result<Response> {
        let me = self.lease_nick()?;
        let now = now_ms();
        let turns: Vec<leases::Turn> = self
            .leases
            .leases
            .mine(&me, now)
            .into_iter()
            .filter(|turn| {
                turn.want.session == session && roles::of_lease(&turn.want.name).is_some()
            })
            .collect();
        if turns.is_empty() {
            bail!("{session} holds no role");
        }
        for turn in turns {
            let role = roles::of_lease(&turn.want.name).unwrap_or_default();
            let text = roles::left_words(session, role);
            let event = LeaseEvent::Release {
                id: turn.want.id,
                at: now,
            };
            self.say_lease(session, event, text);
        }
        Ok(Response::Done)
    }

    /// [`zirc_core::ipc::Request::RoleSay`].
    pub(super) fn role_say(
        &mut self,
        session: &str,
        to: Option<String>,
        reply_to: Option<String>,
        text: &str,
    ) -> Result<Response> {
        let role = self
            .role_of(session)
            .with_context(|| format!("{session} holds no role to speak as"))?;
        anyhow::ensure!(!text.trim().is_empty(), "say something");
        let say = Say {
            role: role.clone(),
            to,
            reply_to,
        };
        let event = say.event(text);
        let message = self.signed(session, event);
        let client = self.connected()?;
        zirc_core::send::send_message(client, &role, &message)
            .with_context(|| format!("couldn't send to {role}"))?;
        Ok(Response::Done)
    }

    /// [`zirc_core::ipc::Request::RoleStanding`].
    pub(super) async fn role_standing(&self) -> Result<Response> {
        let server = self
            .content_server()
            .context("this machine can't ask its organisation's server about roles")?;
        let standing = server.roles().await.map_err(anyhow::Error::msg)?;
        Ok(Response::RoleStanding(standing))
    }

    /// What role `role`'s nick, `from` with `account`, sent this machine, as the line for its
    /// holder's watch: a wake for a session that holds the role here, from the role's own
    /// account, and nothing else.
    pub(super) fn role_woke(
        &self,
        account: Option<&str>,
        from: &str,
        message: &Message,
    ) -> Option<Direct> {
        let (wake, text) = Wake::of(&message.event)?;
        let role = wake.woke.role.as_deref()?;
        let theirs = account.is_some_and(|account| account.eq_ignore_ascii_case(role))
            && from.eq_ignore_ascii_case(role);
        if !theirs || self.role_of(&wake.session).as_deref() != Some(role) {
            tracing::debug!("{from} spoke for role {role}, which isn't its or held here: ignored");
            return None;
        }
        let wrote = wake.woke.what == zirc_proto::woke::What::Wrote;
        Some(Direct {
            from: if wrote {
                wake.woke.from.clone().unwrap_or_else(|| from.to_owned())
            } else {
                from.to_owned()
            },
            from_session: None,
            session: Some(wake.session.clone()),
            text: text.to_owned(),
            query: None,
            answer: None,
            note: false,
            colleague: false,
            why: None,
            id: wake.woke.line.clone(),
            org: None,
            role: Some(role.to_owned()),
            woke: (!wrote).then(|| wake.woke.clone()),
        })
    }

    /// Once a second, for the roles held here: each asked for again well before its turn
    /// ends, while its session's watch lives; and a request that came second to another
    /// session's, once settled, withdrawn and its session told.
    pub(super) fn roles_tick(&mut self) {
        let Some(me) = self.client.as_ref().map(|client| client.nick().to_owned()) else {
            return;
        };
        let now = now_ms();
        let mine: Vec<leases::Turn> = self
            .leases
            .leases
            .mine(&me, now)
            .into_iter()
            .filter(|turn| roles::of_lease(&turn.want.name).is_some())
            .collect();
        for turn in mine {
            let session = turn.want.session.clone();
            let role = roles::of_lease(&turn.want.name)
                .unwrap_or_default()
                .to_owned();
            if turn.start <= now {
                if turn.end.saturating_sub(now) < RENEW_BEFORE {
                    self.renew(&me, &turn, leases::MAX_SECONDS, now);
                }
            } else if now >= turn.want.at + leases::SETTLE {
                let event = LeaseEvent::Release {
                    id: turn.want.id.clone(),
                    at: now,
                };
                self.say_lease(
                    &session,
                    event,
                    format!("{session} came second to role {role}"),
                );
                let holder = self
                    .leases
                    .leases
                    .lease(&turn.want.name, now, &me)
                    .holders
                    .into_iter()
                    .next()
                    .map_or_else(
                        || "another session".to_owned(),
                        |h| format!("{}/{}", h.nick, h.session),
                    );
                self.tell_session(
                    format!("{session}: {holder} took the {role} role a moment before you: you're not the {role}"),
                    None,
                );
            }
        }
    }
}
