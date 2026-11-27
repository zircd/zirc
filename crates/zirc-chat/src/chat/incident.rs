//! Incident cards, as every app draws them ([`zirc_proto::incident`]; the user's choice, 27
//! Nov 2026, mock `FixerAfter`): a coworker's wakes about one alert, build or issue as one
//! card ([`super::RowShape::Incident`]) where its first wake was, with its source chip, a
//! short title, one time, how it stands, the coworker's line on it and a button that shows
//! the wakes themselves. Every word is the core's; apps keep only whether a card is open.

use serde::Serialize;
use ts_rs::TS;
use zirc_proto::incident::{self, Incident};

use super::face::LineSpeaker;

/// What a [`super::RowShape::Incident`] row draws.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct IncidentCard {
    /// Where it came from, in its chip: `ALERTS`, `CI`, `SENTRY`.
    pub source: String,
    /// The theme role its chip is drawn in ([`zirc_proto::woke::Tone::role`]).
    pub tone: String,
    /// What it is, in bold: `Web pod is failing`.
    pub title: String,
    /// How it stands, muted, under the title: `Still open · woke fixer 4 times, last 20:41`;
    /// none once it ended, its pill saying how, having woken once.
    pub status: Option<String>,
    /// What the tool said, cut to its gist, muted, under the status.
    pub detail: Option<String>,
    /// What names it, in monospace after the status: `web-7ff4…`.
    pub reference: Option<String>,
    /// The coworker's line on it, under a rule, with its face; `None` until it said one.
    pub outcome: Option<IncidentOutcome>,
    /// Its wakes, oldest first, shown under it while it's open.
    pub wakes: Vec<IncidentWake>,
    /// What its button says to show them: `Show the 4 wakes`.
    pub show: String,
    /// What it says while they're shown: [`zirc_proto::incident::HIDE`].
    pub hide: String,
    /// Whether it's still open, for a screen reader's label and the outcome's pill.
    pub open: bool,
}

/// The coworker's line on an incident.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct IncidentOutcome {
    /// Who said it: its face, small, before the words.
    pub speaker: Option<LineSpeaker>,
    /// What it said.
    pub text: String,
    /// How it ended, in a pill after the words: `Done`, `Fixed`; none while it's open.
    pub pill: Option<String>,
}

/// One wake of an incident, as its card shows it opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct IncidentWake {
    /// When: `20:41`.
    pub clock: String,
    /// What its line said, whole.
    pub text: String,
}

impl IncidentCard {
    /// The card of `incident`, its coworker called `who`, times said by `clock`, and the
    /// face of whoever said its outcome.
    pub(super) fn of(
        incident: &Incident,
        who: &str,
        clock: impl Fn(i64) -> String,
        speaker: Option<LineSpeaker>,
    ) -> Self {
        Self {
            source: incident.head.source.clone(),
            tone: incident.head.tone.role().to_owned(),
            title: incident.head.title.clone(),
            status: incident.status(who, &clock),
            detail: incident.head.detail.clone(),
            reference: incident.head.reference.clone(),
            outcome: incident.outcome.as_ref().map(|(_, text)| IncidentOutcome {
                speaker,
                text: text.clone(),
                pill: incident.pill(),
            }),
            wakes: incident
                .seen
                .iter()
                .map(|seen| IncidentWake {
                    clock: clock(seen.at_ms),
                    text: seen.words.clone(),
                })
                .collect(),
            show: incident.show(),
            hide: incident::HIDE.to_owned(),
            open: incident.end.is_none(),
        }
    }

    /// What it says, whole, for copying and a screen reader: its source, title, status,
    /// detail and outcome.
    #[must_use]
    pub fn said(&self) -> String {
        [
            Some(format!("{}: {}", self.source, self.title)),
            self.status.clone(),
            self.detail.clone(),
            self.outcome.as_ref().map(|outcome| outcome.text.clone()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(". ")
    }
}
