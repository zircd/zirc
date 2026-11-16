//! A team's own hints ([`zirc_proto::hints`]), kept on its server rather than in its
//! repositories, so one added or removed holds at once on every machine: for a repository
//! (`#<repo>`) or the whole organisation ([`zirc_proto::hints::ORG`]). The content API lists,
//! adds and removes them (`crate::accounts::content`), for the organisation's people and their
//! machines' agents alike; only a person makes one `required`, which no layer above overrides.
//!
//! Every change is logged ([`Change`]), the newest [`LOG`] kept, and the hints and the log are
//! kept in [`KEPT`] beside Ergo's history, written whole and renamed into place.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;
use zirc_proto::hints::{Action, Change, Hint, Listing, ORG, Stored, check, is_scope, layer_of};

/// The file beside Ergo's history the hints are kept in.
pub const KEPT: &str = "zirc-hints.json";

/// How many changes the log keeps, the newest.
pub const LOG: usize = 1000;

/// Who's changing the hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Who {
    /// The account asking: a person's, or one of their machines'.
    pub account: String,
    /// Whether it's a machine's, whose agents may be asking, rather than the person's own.
    pub machine: bool,
    /// The agent session asking, as a machine's request says.
    pub session: Option<String>,
}

/// Why a change was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// It isn't a hint that can be kept, saying why.
    Invalid(String),
    /// There's no such hint.
    NotFound,
    /// It's there already.
    Exists,
}

/// The hints and their log, as kept.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Kept {
    /// The hints and the version they're at.
    #[serde(flatten)]
    pub listing: Listing,
    /// The changes, oldest first.
    #[serde(default)]
    pub log: Vec<Change>,
}

impl Kept {
    /// Adds `hint` for `scope`, as `who`, at `at`: the change made.
    ///
    /// # Errors
    ///
    /// When the scope or the hint won't do, a machine makes it required, or it's there already.
    pub fn add(&mut self, scope: &str, hint: Hint, who: &Who, at: &str) -> Result<Change, Refusal> {
        if !is_scope(scope) {
            return Err(Refusal::Invalid(format!(
                "a hint is for {ORG} or a repository's channel, such as #api, in lower case"
            )));
        }
        let problem = check(std::slice::from_ref(&hint), layer_of(scope));
        if let Some(problem) = problem.into_iter().next() {
            return Err(Refusal::Invalid(problem));
        }
        if hint.required && who.machine {
            return Err(Refusal::Invalid(
                "only a person makes a hint required, on the Playbook page: an agent's can always be overridden"
                    .into(),
            ));
        }
        let id = hint.id();
        if self
            .listing
            .hints
            .iter()
            .any(|kept| kept.id == id && kept.scope == scope)
        {
            return Err(Refusal::Exists);
        }
        let stored = Stored {
            id,
            scope: scope.to_owned(),
            hint,
            by: who.account.clone(),
            session: who.session.clone(),
            at: at.to_owned(),
        };
        self.listing.hints.push(stored.clone());
        Ok(self.logged(Action::Added, stored, who, at))
    }

    /// Removes the hint `id`, as `who`, at `at`: the change made.
    ///
    /// # Errors
    ///
    /// When there's no such hint.
    pub fn remove(&mut self, id: &str, who: &Who, at: &str) -> Result<Change, Refusal> {
        let at_index = self
            .listing
            .hints
            .iter()
            .position(|kept| kept.id == id)
            .ok_or(Refusal::NotFound)?;
        let stored = self.listing.hints.remove(at_index);
        Ok(self.logged(Action::Removed, stored, who, at))
    }

    fn logged(&mut self, action: Action, hint: Stored, who: &Who, at: &str) -> Change {
        self.listing.version += 1;
        let change = Change {
            action,
            hint,
            by: who.account.clone(),
            session: who.session.clone(),
            at: at.to_owned(),
            version: self.listing.version,
        };
        self.log.push(change.clone());
        if self.log.len() > LOG {
            self.log.drain(..self.log.len() - LOG);
        }
        change
    }
}

/// The hints, kept in a file beside Ergo's history.
pub struct Store {
    path: Option<PathBuf>,
    kept: tokio::sync::Mutex<Kept>,
}

impl Store {
    /// The store kept beside `history`, as last saved there.
    #[must_use]
    pub fn beside(history: &Path) -> Self {
        let path = history.with_file_name(KEPT);
        let kept = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            kept: tokio::sync::Mutex::new(kept),
        }
    }

    /// The hints as they are.
    pub async fn listing(&self) -> Listing {
        self.kept.lock().await.listing.clone()
    }

    /// The log, newest first.
    pub async fn log(&self) -> Vec<Change> {
        self.kept.lock().await.log.iter().rev().cloned().collect()
    }

    /// Makes a change with `change` and keeps the result: the change made.
    ///
    /// # Errors
    ///
    /// As `change` refuses.
    pub async fn change(
        &self,
        change: impl FnOnce(&mut Kept) -> Result<Change, Refusal>,
    ) -> Result<Change, Refusal> {
        let mut kept = self.kept.lock().await;
        let made = change(&mut kept)?;
        self.save(&kept);
        Ok(made)
    }

    fn save(&self, kept: &Kept) {
        let Some(path) = &self.path else {
            return;
        };
        let saved = serde_json::to_vec(kept)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                let partial = path.with_extension("json.partial");
                std::fs::write(&partial, bytes)?;
                std::fs::rename(&partial, path)
            });
        if let Err(error) = saved {
            warn!(%error, path = %path.display(), "couldn't keep the team's hints");
        }
    }
}

#[cfg(test)]
mod tests {
    use zirc_proto::hints::{Level, When};

    use super::*;

    fn hint(text: &str) -> Hint {
        Hint {
            kind: "overlap".into(),
            when: When::default(),
            level: Level::Act,
            text: text.into(),
            required: false,
        }
    }

    fn person() -> Who {
        Who {
            account: "ada".into(),
            machine: false,
            session: None,
        }
    }

    fn agent() -> Who {
        Who {
            account: "ada-mbp".into(),
            machine: true,
            session: Some("cc-1".into()),
        }
    }

    const AT: &str = "2026-11-16T10:00:00Z";

    #[test]
    fn people_and_agents_add_and_remove_hints_each_change_logged() {
        let mut kept = Kept::default();
        let added = kept.add("#api", hint("ask first"), &agent(), AT).unwrap();
        assert_eq!((added.action, added.version), (Action::Added, 1));
        assert_eq!(added.hint.session.as_deref(), Some("cc-1"));
        kept.add(ORG, hint("ask first"), &person(), AT).unwrap();
        // The same hint for the same scope is there already; for another it's another.
        assert_eq!(
            kept.add("#api", hint("ask first"), &person(), AT),
            Err(Refusal::Exists)
        );
        assert_eq!(kept.listing.for_scope("#api").len(), 1);
        let removed = kept.remove(&added.hint.id, &person(), AT).unwrap();
        assert_eq!(
            (removed.action, removed.by.as_str()),
            (Action::Removed, "ada")
        );
        assert!(kept.listing.for_scope("#api").is_empty());
        assert_eq!(kept.listing.version, 3);
        assert_eq!(kept.log.len(), 3);
        assert_eq!(kept.remove("nope", &person(), AT), Err(Refusal::NotFound));
    }

    #[test]
    fn refuses_what_wont_do_and_an_agents_required_hint() {
        let mut kept = Kept::default();
        let refused = |result: Result<Change, Refusal>| match result {
            Err(Refusal::Invalid(why)) => why,
            other => panic!("{other:?}"),
        };
        assert!(
            refused(kept.add("api", hint("x"), &person(), AT)).contains("repository's channel")
        );
        let mut odd = hint("x");
        odd.kind = "nope".into();
        assert!(refused(kept.add("#api", odd, &person(), AT)).contains("no such kind"));
        let required = Hint {
            required: true,
            ..hint("land it")
        };
        assert!(
            refused(kept.add("#api", required.clone(), &agent(), AT)).contains("only a person")
        );
        assert!(kept.add("#api", required, &person(), AT).is_ok());
    }

    #[test]
    fn the_log_keeps_the_newest() {
        let mut kept = Kept::default();
        for i in 0..=LOG {
            kept.add("#api", hint(&format!("hint {i}")), &person(), AT)
                .unwrap();
        }
        assert_eq!(kept.log.len(), LOG);
        assert_eq!(kept.log[0].version, 2);
    }

    #[tokio::test]
    async fn a_store_keeps_its_hints_across_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let history = dir.path().join("history.db");
        let store = Store::beside(&history);
        store
            .change(|kept| kept.add("#api", hint("ask first"), &person(), AT))
            .await
            .unwrap();
        let again = Store::beside(&history);
        assert_eq!(again.listing().await, store.listing().await);
        assert_eq!(again.log().await.len(), 1);
    }
}
