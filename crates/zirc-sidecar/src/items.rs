//! Roles' open items ([`zirc_proto::role_report`]): one per role, kind and key, from the
//! reports said in `#ops`, kept until a landing that names it passes or its holder says it's
//! done or baselined.
//!
//! It reads Ergo's history as it's written, by Ergo's own row id from the last one read, as
//! the activity store does ([`crate::activity`]): reports and closings in `#ops`, and CI's
//! landings in `#agents`, whose `Fixes: report <id>` trailers close items. Nothing anyone says
//! is logged. The items, and the row read up to, are kept in a file beside the history
//! ([`path_for`]), so a restart neither wakes anyone again nor misses what came meanwhile.
//!
//! A new item wakes the role's holder; while no one holds the role the wake waits for the
//! next. One left open past its kind's time is said again
//! ([`zirc_proto::role_report::OpenItem::remind_at`]). Whom a role is held by, and how a wake
//! reaches them, is the roles' ([`Wake`] is what's handed on).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::warn;
use zirc_proto::ci::{self, LANDING_KIND};
use zirc_proto::role_report::{self, Closing, End, OpenItem, OpenItems, RoleReport};
use zirc_proto::{Event, Message};

use crate::exporter::{self, Opened};

/// How long a closed item is kept: the dashboard shows what closed today.
pub const KEEP_CLOSED: u64 = 24 * 60 * 60;

/// How often [`keep_items`] reads what's new in the history and what's due.
pub const EVERY: Duration = Duration::from_secs(15);

/// How many history rows one read takes.
const BATCH: i64 = 1_000;

/// The store for the history at `history`: `zirc_items.json` in the same directory.
#[must_use]
pub fn path_for(history: &Path) -> PathBuf {
    history.with_file_name("zirc_items.json")
}

/// A wake for a role's holder, about one of its items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wake {
    /// The role.
    pub role: String,
    /// The item.
    pub item: OpenItem,
    /// Whether it's a reminder of one already said.
    pub reminder: bool,
}

impl Wake {
    /// The wake its role's holder is woken with: `main-fix woke as the fixer: CI found a
    /// flaky test, //a:b · build 612`.
    #[must_use]
    pub fn woke(&self) -> zirc_proto::woke::Woke {
        zirc_proto::woke::Woke::reported(&self.item, self.reminder)
    }

    /// What the holder's watch shows of it ([`zirc_proto::role_report::OpenItem::told`]).
    #[must_use]
    pub fn text(&self) -> String {
        self.item.told(self.reminder)
    }
}

/// A landing that passed closed some of a role's items: what wakes its holder, so the role's
/// conversation says it (`CI landed ops-watch · build 1502 · the item is closed`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closed {
    /// The role.
    pub role: String,
    /// The items it closed, by id.
    pub ids: Vec<String>,
    /// The landing's build.
    pub build: Option<String>,
    /// The branch that landed.
    pub branch: Option<String>,
}

impl Closed {
    /// The wake its role's holder is woken with.
    #[must_use]
    pub fn woke(&self) -> zirc_proto::woke::Woke {
        zirc_proto::woke::Woke::closed_by_landing(
            &self.role,
            self.build.clone(),
            self.branch.clone(),
            u32::try_from(self.ids.len()).unwrap_or(u32::MAX),
        )
    }

    /// What the holder's watch shows of it: the wake's words, and the items' ids.
    #[must_use]
    pub fn text(&self) -> String {
        format!("{} [{}]", self.woke().words(None), self.ids.join(", "))
    }
}

/// What a [`Items::catch_up`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Read {
    /// Whether anything changed.
    pub changed: bool,
    /// The row it stopped before, as Ergo is still writing it: passed over if it's still
    /// there next time.
    pub unplaced: Option<i64>,
}

/// Every role's items, and the history row they're read up to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Items {
    /// The last history row read.
    #[serde(default)]
    pub row: i64,
    /// Open items, oldest first, and those closed within [`KEEP_CLOSED`].
    #[serde(default)]
    pub items: Vec<OpenItem>,
    /// What landings closed since [`Items::due`] last said it. Not kept: a restart's
    /// catch-up reads only what's new.
    #[serde(skip)]
    pub closed: Vec<Closed>,
    /// Every item closed since [`Items::due`] last said it, however it ended, by role and id:
    /// a wake waiting for its role's next holder goes with it ([`keep_items`]).
    #[serde(skip)]
    pub ended: Vec<(String, String)>,
}

impl Items {
    /// What's kept at `path`, none when there's nothing there or it can't be read.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Keeps them at `path`, whole or not at all.
    ///
    /// # Errors
    ///
    /// When the file can't be written.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        crate::save_json(path, self)
    }

    /// The open item `id` names for `role`.
    fn open(&mut self, role: &str, id: &str) -> Option<&mut OpenItem> {
        self.items
            .iter_mut()
            .find(|item| item.closed.is_none() && item.report.role == role && item.id == id)
    }

    /// Takes in `message`, said by `account` in `channel` at `now`: a report opens an item or
    /// counts again on the open one, a closing closes one, and CI's landing that passed
    /// closes those its `Fixes:` name. Says whether anything changed.
    pub fn heard(
        &mut self,
        channel: &str,
        account: Option<&str>,
        message: &Message,
        now: u64,
    ) -> bool {
        let ops = channel.eq_ignore_ascii_case(zirc_proto::state::CHANNEL);
        if ops && let Some(report) = RoleReport::of(&message.event) {
            return self.take(report, now);
        }
        if ops && let Some(closing) = Closing::of(&message.event) {
            let closed = self.close(
                &closing.role,
                std::slice::from_ref(&closing.id),
                closing.end,
                now,
            );
            // Said of one closed already, it still drops a wake left waiting about it, as one
            // closed before a sidecar that dropped them.
            if !closed {
                self.ended.push((closing.role, closing.id));
            }
            return closed;
        }
        if channel.eq_ignore_ascii_case(zirc_proto::AGENTS_CHANNEL)
            && ci::speaks_for_ci(account)
            && let Event::Custom { kind, data, .. } = &message.event
            && kind == LANDING_KIND
            && let Ok(data) = serde_json::from_value::<ci::Data>(data.clone())
            && data.state.as_deref() == Some("passed")
            && !data.tried
        {
            let ids = role_report::fixed_by(&data.fixes);
            let mut roles: Vec<String> = self
                .items
                .iter()
                .filter(|item| item.closed.is_none() && ids.contains(&item.id))
                .map(|item| item.report.role.clone())
                .collect();
            roles.dedup();
            let mut changed = false;
            for role in roles {
                let closing: Vec<String> = ids
                    .iter()
                    .filter(|id| {
                        self.items.iter().any(|item| {
                            item.closed.is_none() && item.report.role == role && &item.id == *id
                        })
                    })
                    .cloned()
                    .collect();
                changed |= self.close(&role, &ids, End::Landed, now);
                self.closed.push(Closed {
                    role,
                    ids: closing,
                    build: data.build.clone(),
                    branch: data.branch.clone(),
                });
            }
            return changed;
        }
        false
    }

    /// Takes `report` in at `now`: it opens an item, or counts again on the open one. Says
    /// whether anything changed: nothing, for one that isn't a report.
    pub fn take(&mut self, report: RoleReport, now: u64) -> bool {
        let Ok(report) = report.checked() else {
            return false;
        };
        if let Some(item) = self.open(&report.role, &report.id()) {
            item.reports = item.reports.saturating_add(1);
            item.last = now;
        } else {
            self.items.push(OpenItem::opened(report, now));
        }
        true
    }

    /// Closes `role`'s open items among `ids` as `end` at `now`.
    fn close(&mut self, role: &str, ids: &[String], end: End, now: u64) -> bool {
        let mut changed = false;
        for id in ids {
            if let Some(item) = self.open(role, id) {
                item.closed = Some(now);
                item.end = Some(end);
                changed = true;
                self.ended.push((role.to_owned(), id.clone()));
            }
        }
        changed
    }

    /// What's due at `now`: a wake for each open item its role hasn't been woken for, and a
    /// reminder for each past its time. Each is marked as said, and closed items older than
    /// [`KEEP_CLOSED`] go.
    pub fn due(&mut self, now: u64) -> Vec<Wake> {
        self.items.retain(|item| {
            item.closed
                .is_none_or(|closed| now.saturating_sub(closed) < KEEP_CLOSED)
        });
        let mut wakes = Vec::new();
        for item in self.items.iter_mut().filter(|item| item.closed.is_none()) {
            let reminder = if item.woke {
                if now < item.remind_at() {
                    continue;
                }
                item.reminded = item.reminded.saturating_add(1);
                true
            } else {
                item.woke = true;
                false
            };
            wakes.push(Wake {
                role: item.report.role.clone(),
                item: item.clone(),
                reminder,
            });
        }
        wakes
    }

    /// `role`'s items at `now`, as the dashboard reads them.
    #[must_use]
    pub fn listing(&self, role: &str, now: u64) -> OpenItems {
        let of_role = || self.items.iter().filter(|item| item.report.role == role);
        let mut closed_today: Vec<OpenItem> = of_role()
            .filter(|item| {
                item.closed
                    .is_some_and(|closed| now.saturating_sub(closed) < KEEP_CLOSED)
            })
            .cloned()
            .collect();
        closed_today.sort_by_key(|item| std::cmp::Reverse(item.closed));
        let open = of_role()
            .filter(|item| item.closed.is_none())
            .cloned()
            .collect();
        OpenItems::new(role, open, closed_today)
    }

    /// Takes in every history row after [`Items::row`] in `#ops` and `#agents`, from the
    /// history at `history`, up to one Ergo is still writing (passed over when it's `pass`),
    /// and says what it did. A history whose schema this doesn't know is left unread.
    ///
    /// # Errors
    ///
    /// When the history can't be read.
    pub fn catch_up(&mut self, history: &Path, pass: Option<i64>) -> rusqlite::Result<Read> {
        let Opened::Open(connection) = exporter::open_history(history)? else {
            return Ok(Read::default());
        };
        let channels = [zirc_proto::state::CHANNEL, zirc_proto::AGENTS_CHANNEL];
        let mut read = Read::default();
        loop {
            let after = exporter::messages_after(&connection, self.row, &channels, BATCH, pass)?;
            for each in after.heard {
                read.changed |= self.heard(
                    &each.channel,
                    each.account.as_deref(),
                    &each.message,
                    each.at,
                );
            }
            if let Some(last) = after.last {
                read.changed |= self.row != last;
                self.row = last;
            }
            if after.unplaced.is_some() || after.last.is_none() {
                read.unplaced = after.unplaced;
                return Ok(read);
            }
        }
    }
}

/// The items kept beside the history at `history`, now: what the account endpoint serves.
#[must_use]
pub fn kept(history: &Path) -> Items {
    Items::load(&path_for(history))
}

/// Keeps the items beside the history at `history` until the process ends, with what comes
/// to a role `direct`ly, never said in `#ops` (a new issue, [`zirc_proto::role_report::Subject::in_ops`]): every
/// [`EVERY`], what's new in the history, then what's due, each wake handed to `wake`, what
/// landings closed to `closed`, and each item closed, however, to `ended` (role, id).
pub async fn keep_items<W, C, E>(
    history: PathBuf,
    mut direct: tokio::sync::mpsc::UnboundedReceiver<RoleReport>,
    wake: W,
    closed: C,
    ended: E,
) where
    W: Fn(Wake) + Send + Sync + 'static,
    C: Fn(Closed) + Send + Sync + 'static,
    E: Fn(&str, &str) + Send + Sync + 'static,
{
    let path = path_for(&history);
    let mut items = Items::load(&path);
    let mut unplaced = None;
    loop {
        let read = {
            let mut copy = items.clone();
            let history = history.clone();
            tokio::task::spawn_blocking(move || {
                copy.catch_up(&history, unplaced).map(|read| (copy, read))
            })
            .await
        };
        let mut changed = match read {
            Ok(Ok((read, said))) => {
                items = read;
                unplaced = said.unplaced;
                said.changed
            }
            Ok(Err(error)) => {
                warn!(%error, "couldn't read the history for roles' items");
                false
            }
            Err(error) => {
                warn!(%error, "reading the history for roles' items didn't finish");
                false
            }
        };
        // What came straight to its role, never said in `#ops` (a new issue).
        while let Ok(report) = direct.try_recv() {
            changed |= items.take(report, crate::now());
        }
        let wakes = items.due(crate::now());
        changed |= !wakes.is_empty();
        for each in wakes {
            wake(each);
        }
        for each in std::mem::take(&mut items.closed) {
            closed(each);
        }
        for (role, id) in std::mem::take(&mut items.ended) {
            ended(&role, &id);
        }
        if changed && let Err(error) = items.save(&path) {
            warn!(%error, "couldn't keep roles' items");
        }
        // Until it's time to read again, or a report comes straight to a role.
        tokio::select! {
            () = tokio::time::sleep(EVERY) => {}
            Some(report) = direct.recv() => {
                items.take(report, crate::now());
                if let Err(error) = items.save(&path) {
                    warn!(%error, "couldn't keep roles' items");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zirc_proto::role_report::Subject;

    fn said(event: Event) -> Message {
        Message::new("cc-1", event)
    }

    fn flaky(test: &str) -> Message {
        said(
            RoleReport {
                build: Some("612".into()),
                ..RoleReport::new("fixer", Subject::Flaky, test)
            }
            .event(),
        )
    }

    fn landed(fixes: &[&str], state: &str) -> Message {
        let data = ci::Data {
            state: Some(state.into()),
            fixes: fixes.iter().map(|fix| (*fix).to_owned()).collect(),
            ..ci::Data::default()
        };
        said(Event::Custom {
            kind: LANDING_KIND.into(),
            data: serde_json::to_value(data).unwrap(),
            text: "landed".into(),
        })
    }

    const OPS: &str = "#ops";

    #[test]
    fn the_same_report_again_counts_on_the_open_item_and_wakes_no_one_again() {
        let mut items = Items::default();
        assert!(items.heard(OPS, Some("ci"), &flaky("//a:b"), 10));
        let wakes = items.due(10);
        assert_eq!(wakes.len(), 1);
        assert!(!wakes[0].reminder);
        // Who found what for people; the id only in what the holder's watch is told.
        let woke = wakes[0].woke();
        assert_eq!(woke.words(None), "CI found a flaky test, //a:b · build 612");
        assert!(wakes[0].text().contains(&format!("[{}]", wakes[0].item.id)));
        assert!(items.heard(OPS, Some("ci"), &flaky("//a:b"), 20));
        assert!(items.due(20).is_empty());
        let listing = items.listing("fixer", 20);
        assert_eq!(listing.open.len(), 1);
        assert_eq!(listing.open[0].reports, 2);
        assert_eq!(listing.open[0].last, 20);
    }

    #[test]
    fn an_item_left_open_is_said_again_then_twice_as_late() {
        let mut items = Items::default();
        items.heard(OPS, None, &flaky("//a:b"), 0);
        assert_eq!(items.due(0).len(), 1);
        assert!(items.due(30 * 60 - 1).is_empty());
        let wakes = items.due(30 * 60);
        assert!(wakes[0].reminder);
        assert!(wakes[0].woke().words(None).starts_with("still open: "));
        assert!(items.due(60 * 60 - 1).is_empty());
        assert_eq!(items.due(60 * 60).len(), 1);
    }

    #[test]
    fn a_passing_landing_from_ci_naming_it_closes_it_and_another_doesnt() {
        let mut items = Items::default();
        items.heard(OPS, None, &flaky("//a:b"), 0);
        let id = items.items[0].id.clone();
        let fix = format!("report {id}");
        assert!(!items.heard("#agents", Some("zirc-ci"), &landed(&[&fix], "failed"), 1));
        assert!(!items.heard("#agents", Some("ada"), &landed(&[&fix], "passed"), 1));
        assert!(items.heard("#agents", Some("zirc-ci"), &landed(&[&fix], "passed"), 2));
        let listing = items.listing("fixer", 3);
        assert!(listing.open.is_empty());
        assert_eq!(listing.closed_today[0].end, Some(End::Landed));
        assert_eq!(listing.closed_today_words, "Closed today: 1 (fixed)");
        // Reported again once it's closed, it's a new item.
        items.heard(OPS, None, &flaky("//a:b"), 4);
        assert_eq!(items.listing("fixer", 4).open.len(), 1);
        // Closed items go after a day.
        items.due(2 + KEEP_CLOSED);
        assert!(
            items
                .listing("fixer", 2 + KEEP_CLOSED)
                .closed_today
                .is_empty()
        );
    }

    #[test]
    fn a_landing_that_closes_an_item_wakes_its_role_saying_so() {
        let mut items = Items::default();
        items.heard(OPS, None, &flaky("//a:b"), 0);
        let id = items.items[0].id.clone();
        let fix = format!("report {id}");
        items.heard("#agents", Some("zirc-ci"), &landed(&[&fix], "passed"), 2);
        let closed = std::mem::take(&mut items.closed);
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].ids, std::slice::from_ref(&id));
        assert!(
            closed[0]
                .woke()
                .words(None)
                .ends_with("· the item is closed")
        );
    }

    #[test]
    fn every_item_closed_is_said_once_by_role_and_id() {
        let mut items = Items::default();
        items.heard(OPS, None, &flaky("//a:b"), 0);
        let id = items.items[0].id.clone();
        let closing = Closing {
            role: "fixer".into(),
            id: id.clone(),
            end: End::Done,
        };
        items.heard(OPS, Some("m3-mac"), &said(closing.event()), 1);
        assert_eq!(std::mem::take(&mut items.ended), [("fixer".to_owned(), id)]);
        // Said again once it's closed, it changes nothing but is said again, so a wake left
        // waiting about it goes too.
        assert!(!items.heard(OPS, Some("m3-mac"), &said(closing.event()), 2));
        assert_eq!(items.ended.len(), 1);
    }

    #[test]
    fn the_holder_closes_one_as_done_or_baselined() {
        let mut items = Items::default();
        items.heard(OPS, None, &flaky("//a:b"), 0);
        let closing = Closing {
            role: "fixer".into(),
            id: items.items[0].id.clone(),
            end: End::Baselined,
        };
        assert!(items.heard(OPS, Some("m3-mac"), &said(closing.event()), 1));
        assert_eq!(
            items.listing("fixer", 1).closed_today[0].end,
            Some(End::Baselined)
        );
        assert!(items.due(1_000_000).is_empty());
    }

    #[test]
    fn reports_elsewhere_than_ops_are_not_taken() {
        let mut items = Items::default();
        assert!(!items.heard("#general", None, &flaky("//a:b"), 0));
    }

    #[test]
    fn they_keep_across_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_for(&dir.path().join("history.db"));
        let mut items = Items::default();
        items.heard(OPS, None, &flaky("//a:b"), 0);
        items.row = 42;
        items.save(&path).unwrap();
        assert_eq!(Items::load(&path), items);
    }

    #[test]
    fn the_history_is_read_up_to_a_row_ergo_is_still_writing() {
        use crate::trim::testing::{add, add_message, history};
        let dir = tempfile::tempdir().unwrap();
        let path = history(dir.path());
        let said = |message: &Message| zirc_proto::encode(message).unwrap();
        add(
            &path,
            "m1",
            "#ops",
            1_000_000_000,
            "zirc-ci",
            &said(&flaky("//a:b")),
        );
        add(
            &path,
            "m2",
            "#general",
            2_000_000_000,
            "ada",
            &said(&flaky("//c:d")),
        );
        let unplaced = add_message(&path, "m3", "ada", "half written");
        add(
            &path,
            "m4",
            "#ops",
            4_000_000_000,
            "zirc-ci",
            &said(&flaky("//e:f")),
        );
        let mut items = Items::default();
        let read = items.catch_up(&path, None).unwrap();
        assert_eq!((read.changed, read.unplaced), (true, Some(unplaced)));
        assert_eq!(
            items.listing("fixer", 1).open.len(),
            1,
            "#ops's before the unplaced row"
        );
        assert_eq!(items.items[0].opened, 1);
        // Still unplaced next time, it's passed over.
        let read = items.catch_up(&path, Some(unplaced)).unwrap();
        assert_eq!(read.unplaced, None);
        assert_eq!(items.listing("fixer", 4).open.len(), 2);
        assert!(!items.catch_up(&path, None).unwrap().changed);
    }
}
