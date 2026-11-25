//! Reports: how work reaches a role (step 2, the user's choices of 24 Nov 2026). Something
//! that needs a role's holder, such as a new warning in CI's logs, a flaky test or a
//! container restarting, is said once as a typed report ([`RoleReport`], an
//! [`Event::Custom`] of [`KIND`] in the organisation's `#ops`), by `zirc report <role>`, by CI
//! through zirc.dev (`ops/say.sh report`), or by the alert bridge.
//!
//! The tenant's sidecar keeps one open item ([`OpenItem`]) per role, kind and key: the same
//! thing reported again adds nothing but a count, and wakes no one. A new item wakes the
//! role's holder; while no one holds it, it waits for the next. One still open after its
//! kind's time ([`Subject::remind_after`]) is said again, each time twice as late. It closes
//! when a landing that names it passes (a `Fixes: report <id>` trailer, [`fixed_by`]), or
//! when the holder says it's done or baselined ([`Closing`], [`CLOSED_KIND`], `zirc report
//! <role> --done`). zirc.dev's Roles tab reads the open ones and those closed today from the
//! sidecar ([`ITEMS_PATH`], [`OpenItems`]).
//!
//! Every word a wake or a card says of a report is decided here ([`RoleReport::words`]), so
//! no app or script types its own.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

use crate::Event;

/// The kind of a report's event.
pub const KIND: &str = "x-zirc/report";

/// The kind of the event that closes an open item: its holder said it's done or baselined.
pub const CLOSED_KIND: &str = "x-zirc/report-closed";

/// The sidecar's account endpoint for a role's items ([`OpenItems`]): `GET
/// /v1/roles/<role>/items`.
pub const ITEMS_PATH: &str = "/v1/roles/{role}/items";

/// The role CI's and the cluster's reports go to: zirc's own fixer, as an organisation
/// defines it on zirc.dev. `ops/lib.sh` has a copy, as a shell script can't read this.
pub const FIXER: &str = "fixer";

/// What a `Fixes:` trailer says before an item's id to close it when the landing passes:
/// `Fixes: report 3f2a1b9c`.
pub const FIXES_PREFIX: &str = "report ";

/// The most characters a key keeps: a warning's whole line is its key, and a key rides an
/// IRC line beside the rest.
pub const KEY_MAX: usize = 300;

/// The most characters a summary, or details, keep.
pub const SUMMARY_MAX: usize = 300;

/// The most characters a build's name keeps.
pub const BUILD_MAX: usize = 32;

/// What a role's card says of an open item whose holder was already reminded of it
/// ([`OpenItem::reminded`]): `build 609 · 2 h · nudged`.
pub const NUDGED: &str = "nudged";

/// What a report is about. Kinds are added here; a sidecar older than a kind doesn't read its
/// reports, as it couldn't say what they are.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
#[ts(rename = "RoleReportSubject")]
pub enum Subject {
    /// A new warning or deprecation notice in CI's logs: its key is the warning's line, as
    /// `ops/lib.sh`'s `warnings` prints it.
    Warning,
    /// A test that failed, then passed when run again: its key is the test.
    Flaky,
    /// A container that restarted, or a tenant's server that stays not ready: its key is
    /// the workload, `<namespace>/<name>`.
    Container,
}

impl Subject {
    /// Every kind.
    pub const ALL: [Self; 3] = [Self::Warning, Self::Flaky, Self::Container];

    /// How it's written: `warning`, `flaky`, `container`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Warning => "warning",
            Self::Flaky => "flaky",
            Self::Container => "container",
        }
    }

    /// The kind `name` is, as [`Subject::as_str`] writes it.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }

    /// What a wake or a card calls a new one: `new warning`, `flaky test`.
    #[must_use]
    pub const fn phrase(self) -> &'static str {
        match self {
            Self::Warning => "new warning",
            Self::Flaky => "flaky test",
            Self::Container => "container",
        }
    }

    /// What a role's form and card call the kind: `a new warning`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Warning => "a new warning",
            Self::Flaky => "a flaky test",
            Self::Container => "a container or tenant",
        }
    }

    /// How long an item of this kind stays open before its holder is reminded, in seconds:
    /// a warning's two hours, as nothing is broken yet; a flaky test's or a container's half
    /// an hour (the nudges of the workflows they replace).
    #[must_use]
    pub const fn remind_after(self) -> u64 {
        match self {
            Self::Warning => 2 * 60 * 60,
            Self::Flaky | Self::Container => 30 * 60,
        }
    }
}

/// A report to a role: what an [`Event::Custom`] of [`KIND`] carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
pub struct RoleReport {
    /// The role it's for, such as `fixer`.
    pub role: String,
    /// What it's about.
    pub kind: Subject,
    /// What makes it the same as an earlier one: a warning's line, a test, a workload.
    pub key: String,
    /// What it says, in a line; the key when its sender says nothing more.
    pub summary: String,
    /// Where to read more, such as the build's log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub link: Option<String>,
    /// More of what's known, such as a container's exit code and restarts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub details: Option<String>,
    /// The CI build it was seen in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub build: Option<String>,
}

/// Why a report can't be sent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refused {
    /// The role isn't a role's name ([`crate::names::role_problem`] says why).
    #[error("{0}")]
    Role(String),
    /// Its key is empty.
    #[error("a report needs a key: what makes it the same as an earlier one")]
    Key,
}

impl RoleReport {
    /// A report to `role` of `kind` about `key`, its summary the key.
    #[must_use]
    pub fn new(role: &str, kind: Subject, key: &str) -> Self {
        Self {
            role: role.to_owned(),
            kind,
            key: key.to_owned(),
            summary: String::new(),
            link: None,
            details: None,
            build: None,
        }
        .tidied()
    }

    /// Itself, each field on one line and cut to its length, and its summary its key when it
    /// has none.
    #[must_use]
    pub fn tidied(self) -> Self {
        let key = crate::clip_line(&self.key, KEY_MAX);
        let summary = match crate::clip_line(&self.summary, SUMMARY_MAX) {
            summary if summary.is_empty() => crate::clip_line(&key, SUMMARY_MAX),
            summary => summary,
        };
        let some = |text: Option<String>, most: usize| {
            text.map(|text| crate::clip_line(&text, most))
                .filter(|text| !text.is_empty())
        };
        Self {
            role: self.role.trim().to_lowercase(),
            kind: self.kind,
            summary,
            link: some(self.link, SUMMARY_MAX),
            details: some(self.details, SUMMARY_MAX),
            build: some(self.build, BUILD_MAX),
            key,
        }
    }

    /// Itself tidied ([`RoleReport::tidied`]), if it may be sent.
    ///
    /// # Errors
    ///
    /// When its role or key isn't one: [`Refused`].
    pub fn checked(self) -> Result<Self, Refused> {
        let report = self.tidied();
        if let Some(problem) = crate::names::role_problem(&report.role) {
            return Err(Refused::Role(problem));
        }
        if report.key.is_empty() {
            return Err(Refused::Key);
        }
        Ok(report)
    }

    /// The id of the item it opens or adds to ([`item_id`]).
    #[must_use]
    pub fn id(&self) -> String {
        item_id(&self.role, self.kind, &self.key)
    }

    /// What a wake or a card says of it: `new warning in build 612: unused import`.
    #[must_use]
    pub fn words(&self) -> String {
        let build = self
            .build
            .as_ref()
            .map(|build| format!(" in build {build}"))
            .unwrap_or_default();
        format!("{}{build}: {}", self.kind.phrase(), self.summary)
    }

    /// Its event, whose text plain IRC clients show: its words, and how to close it. It
    /// names no session, so it wakes no watch by itself: the sidecar wakes the holder.
    ///
    /// # Panics
    /// Never: a report always serialises.
    #[must_use]
    pub fn event(&self) -> Event {
        let link = self
            .link
            .as_ref()
            .map(|link| format!(" {link}"))
            .unwrap_or_default();
        Event::Custom {
            kind: KIND.to_owned(),
            data: serde_json::to_value(self).expect("a report serialises"),
            text: format!(
                "report to {} [{}]: {}{link}",
                self.role,
                self.id(),
                self.words()
            ),
        }
    }

    /// The report `event` says, if it's one.
    #[must_use]
    pub fn of(event: &Event) -> Option<Self> {
        match event {
            Event::Custom { kind, data, .. } if kind == KIND => {
                serde_json::from_value::<Self>(data.clone())
                    .ok()
                    .map(Self::tidied)
            }
            _ => None,
        }
    }
}

/// The id of the one item a role keeps for `kind` and `key`: eight hex digits, the same
/// wherever it's worked out, so a landing's `Fixes: report <id>` and `zirc report --done`
/// name it without asking the sidecar.
#[must_use]
pub fn item_id(role: &str, kind: Subject, key: &str) -> String {
    let mut hash = Sha256::new();
    for part in [role, kind.as_str(), key] {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    crate::hex(&hash.finalize()[..4])
}

/// The trailer a landing's commit closes item `id` with, when it passes: `Fixes: report <id>`.
#[must_use]
pub fn fixes_trailer(id: &str) -> String {
    format!("Fixes: {FIXES_PREFIX}{id}")
}

/// The kinds, as a reader is told them: `warning, flaky or container`.
#[must_use]
pub fn kinds_said() -> String {
    let names: Vec<&str> = Subject::ALL.iter().map(|kind| kind.as_str()).collect();
    match names.split_last() {
        Some((last, [])) => (*last).to_owned(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// How a wake and its holder's text start, for an item with `words` and `id`, new or as a
/// reminder: `new warning in build 612: unused import [3f2a1b9c]`, `still open: …`.
#[must_use]
pub fn head(words: &str, id: &str, reminder: bool) -> String {
    let still = if reminder { "still open: " } else { "" };
    format!("{still}{words} [{id}]")
}

/// The ids of the items a landing's `Fixes:` trailers close ([`crate::ci::Data::fixes`]),
/// each said as `report <id>`.
#[must_use]
pub fn fixed_by(fixes: &[String]) -> Vec<String> {
    fixes
        .iter()
        .filter_map(|fix| fix.strip_prefix(FIXES_PREFIX))
        .map(|id| id.trim().to_lowercase())
        .filter(|id| id.len() == 8 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        .collect()
}

/// How an item ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[ts(rename = "RoleReportEnd")]
pub enum End {
    /// A landing that names it passed.
    Landed,
    /// Its holder said it's done.
    Done,
    /// Its holder added it to the baseline: it can't be fixed here, as a dependency's own
    /// warning.
    Baselined,
}

impl End {
    /// What a card says of it.
    #[must_use]
    pub const fn words(self) -> &'static str {
        match self {
            Self::Landed => "fixed",
            Self::Done => "done",
            Self::Baselined => "to the baseline",
        }
    }

    /// Every end, in the order a count of them is said.
    pub const ALL: [Self; 3] = [Self::Landed, Self::Done, Self::Baselined];
}

/// The holder says an item is done or baselined: what an [`Event::Custom`] of
/// [`CLOSED_KIND`] carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "RoleReportClosing")]
pub struct Closing {
    /// The role it's for.
    pub role: String,
    /// The item's id ([`item_id`]).
    pub id: String,
    /// How it ended: done or baselined, never landed, which only CI says.
    pub end: End,
}

impl Closing {
    /// Its event.
    ///
    /// # Panics
    /// Never: a closing always serialises.
    #[must_use]
    pub fn event(&self) -> Event {
        Event::Custom {
            kind: CLOSED_KIND.to_owned(),
            data: serde_json::to_value(self).expect("a closing serialises"),
            text: format!("{} [{}]: {}", self.role, self.id, self.end.words()),
        }
    }

    /// The closing `event` says, if it's one.
    #[must_use]
    pub fn of(event: &Event) -> Option<Self> {
        match event {
            Event::Custom { kind, data, .. } if kind == CLOSED_KIND => {
                serde_json::from_value::<Self>(data.clone())
                    .ok()
                    .filter(|closing| closing.end != End::Landed)
            }
            _ => None,
        }
    }
}

/// Whether `kind` is a report's or a closing's: neither wakes a watch by itself.
#[must_use]
pub fn is_kind(kind: &str) -> bool {
    kind == KIND || kind == CLOSED_KIND
}

/// One thing a role has to do, as the sidecar keeps it: the first report of its kind and key,
/// with how often it's been reported since.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OpenItem {
    /// Its id ([`item_id`]).
    pub id: String,
    /// The first report of it; later ones only count.
    pub report: RoleReport,
    /// What the dashboard's card says of it ([`RoleReport::words`]), so no app words its own.
    #[serde(default)]
    pub words: String,
    /// When it was first reported, in unix seconds.
    #[ts(type = "number")]
    pub opened: u64,
    /// When it was last reported.
    #[ts(type = "number")]
    pub last: u64,
    /// How many times it's been reported.
    pub reports: u32,
    /// Whether the role's holder was woken for it: while no one holds the role, the wake
    /// waits for the next.
    #[serde(default)]
    pub woke: bool,
    /// How many times its holder was reminded.
    #[serde(default)]
    pub reminded: u32,
    /// When it closed, in unix seconds, and how; `None` while it's open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub closed: Option<u64>,
    /// How it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub end: Option<End>,
}

impl OpenItem {
    /// An item opened by `report` at `now`.
    #[must_use]
    pub fn opened(report: RoleReport, now: u64) -> Self {
        Self {
            id: report.id(),
            words: report.words(),
            report,
            opened: now,
            last: now,
            reports: 1,
            woke: false,
            reminded: 0,
            closed: None,
            end: None,
        }
    }

    /// What its role's holder is told of it, new or as a reminder: what was reported, its
    /// id, where to read more and how to close it. `flaky test in build 612: //a:b [3f2a1b9c]
    /// https://ci/612; close it with the trailer Fixes: report 3f2a1b9c`.
    #[must_use]
    pub fn told(&self, reminder: bool) -> String {
        let link = self
            .report
            .link
            .as_ref()
            .map(|link| format!(" {link}"))
            .unwrap_or_default();
        format!(
            "{}{link}; close it with the trailer {}",
            head(&self.words, &self.id, reminder),
            fixes_trailer(&self.id)
        )
    }

    /// When its holder is next reminded, in unix seconds: its kind's time after it opened,
    /// then twice as long after each reminder.
    #[must_use]
    pub fn remind_at(&self) -> u64 {
        let after = self.report.kind.remind_after();
        let doubled = after.saturating_mul(1u64 << self.reminded.min(16));
        self.opened.saturating_add(doubled)
    }
}

/// A role's items, as the sidecar serves them at [`ITEMS_PATH`]: those open, oldest first,
/// and those closed in the last day, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OpenItems {
    /// The role.
    pub role: String,
    /// What's open.
    pub open: Vec<OpenItem>,
    /// What closed in the last day.
    pub closed_today: Vec<OpenItem>,
    /// What the card says of them ([`closed_words`]): `Closed today: 3 (2 fixed, 1 to the
    /// baseline)`, empty when none did.
    #[serde(default)]
    pub closed_today_words: String,
}

impl OpenItems {
    /// `role`'s items: those `open`, and those `closed_today`, with what's said of the latter.
    #[must_use]
    pub fn new(role: &str, open: Vec<OpenItem>, closed_today: Vec<OpenItem>) -> Self {
        Self {
            role: role.to_owned(),
            closed_today_words: closed_words(&closed_today),
            open,
            closed_today,
        }
    }
}

/// What a card says of the items `closed`: `Closed today: 3 (2 fixed, 1 to the baseline)`, or
/// `Closed today: 1 (fixed)`; empty when there are none.
#[must_use]
pub fn closed_words(closed: &[OpenItem]) -> String {
    if closed.is_empty() {
        return String::new();
    }
    let ends: Vec<String> = End::ALL
        .into_iter()
        .filter_map(|end| {
            let n = closed.iter().filter(|item| item.end == Some(end)).count();
            (n > 0).then(|| match closed.len() {
                1 => end.words().to_owned(),
                _ => format!("{n} {}", end.words()),
            })
        })
        .collect();
    format!("Closed today: {} ({})", closed.len(), ends.join(", "))
}

/// These as TypeScript, for the web app's generated file.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        Subject::decl(config),
        RoleReport::decl(config),
        End::decl(config),
        Closing::decl(config),
        OpenItem::decl(config),
        OpenItems::decl(config),
        format!("const ROLE_ITEMS_PATH = {ITEMS_PATH:?};"),
        format!("const ROLE_ITEM_NUDGED = {NUDGED:?};"),
        format!(
            "const ROLE_REPORT_SUBJECTS: readonly {{ id: RoleReportSubject; phrase: string; label: string }}[] = [{}];",
            Subject::ALL
                .iter()
                .map(|kind| format!(
                    "{{ id: {:?}, phrase: {:?}, label: {:?} }}",
                    kind.as_str(),
                    kind.phrase(),
                    kind.label()
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warning() -> RoleReport {
        RoleReport {
            build: Some("612".into()),
            link: Some("https://ci/612".into()),
            ..RoleReport::new("fixer", Subject::Warning, "warning: unused import `N`")
        }
    }

    #[test]
    fn a_report_says_its_kind_build_and_summary() {
        assert_eq!(
            warning().words(),
            "new warning in build 612: warning: unused import `N`"
        );
    }

    #[test]
    fn a_report_reads_back_from_its_event() {
        let report = warning();
        assert_eq!(RoleReport::of(&report.event()), Some(report));
    }

    #[test]
    fn the_same_kind_and_key_is_the_same_item_wherever_its_id_is_worked_out() {
        let id = warning().id();
        assert_eq!(id.len(), 8);
        assert_eq!(
            id,
            item_id("fixer", Subject::Warning, "warning: unused import `N`")
        );
        assert_ne!(
            id,
            item_id("fixer", Subject::Flaky, "warning: unused import `N`")
        );
        assert_ne!(
            id,
            item_id("other", Subject::Warning, "warning: unused import `N`")
        );
    }

    #[test]
    fn a_kind_this_zirc_doesnt_know_isnt_read() {
        let event = Event::Custom {
            kind: KIND.into(),
            data: serde_json::json!({"role": "fixer", "kind": "disk-full", "key": "/data", "summary": ""}),
            text: String::new(),
        };
        assert_eq!(RoleReport::of(&event), None);
        assert_eq!(Subject::named("flaky"), Some(Subject::Flaky));
    }

    #[test]
    fn a_report_is_one_line_cut_short_and_needs_a_role_and_key() {
        let long = "x ".repeat(400);
        let report = RoleReport::new("Fixer", Subject::Flaky, &long);
        assert_eq!(report.role, "fixer");
        assert_eq!(report.key.chars().count(), KEY_MAX);
        assert!(report.key.ends_with('…'));
        assert_eq!(
            RoleReport::new("", Subject::Flaky, "a").checked(),
            Err(Refused::Role(crate::names::role_problem("").unwrap()))
        );
        assert_eq!(
            RoleReport::new("fixer", Subject::Flaky, " ").checked(),
            Err(Refused::Key)
        );
    }

    #[test]
    fn the_kinds_and_a_trailer_are_said_once() {
        assert_eq!(kinds_said(), "warning, flaky or container");
        assert_eq!(fixes_trailer("3f2a1b9c"), "Fixes: report 3f2a1b9c");
        assert_eq!(
            fixed_by(&[fixes_trailer("3f2a1b9c")["Fixes: ".len()..].to_owned()]),
            vec!["3f2a1b9c".to_owned()]
        );
    }

    #[test]
    fn a_landings_fixes_name_the_items_it_closes() {
        let fixes = vec![
            "0123456789abcdef0123456789abcdef01234567".to_owned(),
            "build 12".to_owned(),
            "report 3F2A1B9C".to_owned(),
            "report nope".to_owned(),
        ];
        assert_eq!(fixed_by(&fixes), vec!["3f2a1b9c".to_owned()]);
    }

    #[test]
    fn a_reminder_comes_after_its_kinds_time_then_twice_as_late() {
        let mut item = OpenItem::opened(warning(), 1_000);
        assert_eq!(item.remind_at(), 1_000 + 2 * 3600);
        item.reminded = 1;
        assert_eq!(item.remind_at(), 1_000 + 4 * 3600);
        let flaky = OpenItem::opened(RoleReport::new("fixer", Subject::Flaky, "//a:b"), 0);
        assert_eq!(flaky.remind_at(), 30 * 60);
    }

    #[test]
    fn a_closing_reads_back_but_never_as_landed() {
        let closing = Closing {
            role: "fixer".into(),
            id: "3f2a1b9c".into(),
            end: End::Baselined,
        };
        assert_eq!(Closing::of(&closing.event()), Some(closing.clone()));
        let landed = Closing {
            end: End::Landed,
            ..closing
        };
        assert_eq!(Closing::of(&landed.event()), None);
    }

    #[test]
    fn what_closed_today_is_counted_by_how_it_ended() {
        let closed = |end| OpenItem {
            closed: Some(1),
            end: Some(end),
            ..OpenItem::opened(warning(), 0)
        };
        assert_eq!(closed_words(&[]), "");
        assert_eq!(
            closed_words(&[closed(End::Landed)]),
            "Closed today: 1 (fixed)"
        );
        let items = OpenItems::new(
            "fixer",
            Vec::new(),
            vec![
                closed(End::Baselined),
                closed(End::Landed),
                closed(End::Landed),
            ],
        );
        assert_eq!(
            items.closed_today_words,
            "Closed today: 3 (2 fixed, 1 to the baseline)"
        );
    }

    #[test]
    fn the_holder_is_told_what_it_is_and_how_to_close_it() {
        let item = OpenItem::opened(warning(), 0);
        let told = item.told(false);
        assert!(told.starts_with("new warning in build 612: warning: unused import `N` ["));
        assert!(told.ends_with(&format!(
            "] https://ci/612; close it with the trailer Fixes: report {}",
            item.id
        )));
        assert!(item.told(true).starts_with("still open: new warning"));
    }
}
