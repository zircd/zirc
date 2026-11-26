//! The gotchas zircd hears in the organisation's gotchas room ([`zirc_core::gotchas`]), by
//! the repository each says, kept
//! across restarts, and told to a session once as it starts changing files under their
//! paths without an intent of its own: the moment zircd says one for it
//! ([`super::auto_intents`]), before it's deep in. `zirc intend` says them for a session
//! that declares its own. A gotcha posted is nobody's news otherwise: it wakes no one, nor
//! does its retiring, which zircd hears to forget it.

use std::collections::{BTreeMap, BTreeSet};

use zirc_core::gotchas::{self, Gotcha};
use zirc_core::ipc::{Delivery, Response};
use zirc_core::paths;
use zirc_proto::activity::{self, GotchaTold, Record as _};
use zirc_proto::{Event, Message};

use super::{Daemon, WATCHER_SESSION, read_saved, save_saved};

/// How many of a repository's gotchas zircd keeps, the newest.
const KEPT: usize = 500;

/// By the scope of the repository each is about ([`zirc_core::ipc::Delivery::scope`]), in
/// lowercase: what was heard, oldest first.
pub(super) type Kept = BTreeMap<String, Vec<Gotcha>>;

/// Which gotchas each session was told, by session and the gotcha's text and paths.
pub(super) type Told = BTreeSet<(String, String)>;

/// What a gotcha is, for telling it once.
fn key(gotcha: &Gotcha) -> String {
    format!("{}\t{}", gotcha.paths.join(","), gotcha.text)
}

impl Daemon {
    /// Keeps the gotcha `delivery` says, heard in the gotchas room, live or replayed, or
    /// forgets the one it retires, for the repository it says.
    pub(super) fn remember_gotcha(&mut self, delivery: &Delivery) {
        if !gotchas::is_room(&delivery.channel) || delivery.message.repo.is_none() {
            return;
        }
        let room = self
            .gotchas
            .entry(delivery.scope().to_ascii_lowercase())
            .or_default();
        if let Some(retired) = gotchas::retired(delivery) {
            let before = room.len();
            room.retain(|gotcha| !retired.retires(gotcha));
            if room.len() != before {
                self.save_gotchas();
            }
            return;
        }
        let Some(gotcha) = gotchas::of(delivery) else {
            return;
        };
        // Its echo, or the history read again: once is enough, though the echo may bring the
        // id this machine's own lacked as it was said.
        if let Some(kept) = room.iter_mut().find(|kept| key(kept) == key(&gotcha)) {
            if kept.id.is_none() && gotcha.id.is_some() {
                kept.id = gotcha.id;
                self.save_gotchas();
            }
            return;
        }
        room.push(gotcha);
        if room.len() > KEPT {
            room.drain(..room.len() - KEPT);
        }
        self.save_gotchas();
    }

    /// Tells `session`, as it starts changing `dirs` in the repository whose scope is
    /// `channel`, the gotchas under them it wasn't told yet.
    pub(super) fn tell_gotchas(&mut self, channel: &str, session: &str, dirs: &[String]) {
        let Some(kept) = self.gotchas.get(&channel.to_ascii_lowercase()) else {
            return;
        };
        let told = &self.gotchas_told;
        let new: Vec<&Gotcha> = gotchas::under(kept, dirs)
            .into_iter()
            .filter(|gotcha| !told.contains(&(session.to_owned(), key(gotcha))))
            .collect();
        if new.is_empty() {
            return;
        }
        let keys: Vec<String> = new.iter().map(|gotcha| key(gotcha)).collect();
        // Those it's shown, for how often each is used, by id.
        let ids: Vec<String> = new
            .iter()
            .take(gotchas::SHOWN_AT_START)
            .filter_map(|gotcha| gotcha.id.clone())
            .collect();
        let body = gotchas::started(session, dirs, &new);
        for key in keys {
            self.gotchas_told.insert((session.to_owned(), key));
        }
        for gotcha in ids {
            let told = GotchaTold {
                gotcha,
                how: activity::Told::Start,
            };
            self.record(session, told.event());
        }
        self.save_gotchas_told();
        let me = self.client.as_ref().map(|client| client.nick().to_owned());
        self.tell_watchers(&Response::Delivery(Delivery::new(
            channel,
            me.unwrap_or_default(),
            Message::new(WATCHER_SESSION, Event::Text { body }),
        )));
    }

    /// Takes in the gotchas kept before zircd last stopped.
    pub(super) fn restore_gotchas(&mut self) {
        if let Some(kept) = read_saved::<Kept>(&paths::gotchas(&self.org.state)) {
            self.gotchas = kept;
        }
        // Which were told is kept too: a restart, as at every update, once told each session
        // all of them again.
        if let Some(told) = read_saved::<Told>(&paths::gotchas_told(&self.org.state)) {
            self.gotchas_told = told;
        }
    }

    /// Saves which gotchas each session was told, for after a restart.
    pub(super) fn save_gotchas_told(&self) {
        let path = paths::gotchas_told(&self.org.state);
        if let Err(err) = save_saved(&path, &self.gotchas_told) {
            tracing::warn!("couldn't save which gotchas were told: {err}");
        }
    }

    /// Saves the gotchas heard, for after a restart.
    fn save_gotchas(&self) {
        if let Err(err) = save_saved(&paths::gotchas(&self.org.state), &self.gotchas) {
            tracing::warn!("couldn't save the gotchas: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restart_remembers_which_gotchas_each_session_was_told() {
        let home = tempfile::tempdir().unwrap();
        let daemon = || {
            let org = super::super::Org {
                name: "zirc".into(),
                first: true,
                state: home.path().to_owned(),
                several: false,
            };
            let (inputs, _received) = tokio::sync::mpsc::unbounded_channel();
            Daemon::new(
                home.path().to_owned(),
                org,
                "irc.example".into(),
                None,
                inputs,
            )
        };
        let told = ("cc-1".to_owned(), "web\tsplit it first".to_owned());
        let mut before = daemon();
        before.gotchas_told.insert(told.clone());
        before.save_gotchas_told();
        let mut after = daemon();
        assert!(after.gotchas_told.is_empty());
        after.restore_gotchas();
        assert!(after.gotchas_told.contains(&told));
    }
}
