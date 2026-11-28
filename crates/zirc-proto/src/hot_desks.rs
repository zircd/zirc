//! The office's hot desks: everyone's agent sessions that sit at no coworker's desk, as the
//! apps' Office tab and zirc.dev's Coworkers tab draw them, after the coworkers' badges. A
//! session is in the office while a watch of its is open, and for [`AWAY_SHOWN_SECS`] after
//! (the user's choice, 27 Nov 2026: "remove from view when offline for more than 5 minutes");
//! the rest fold under one line, `29 away` ([`away`]), which opens to show them. What each
//! badge says, and which are shown, is decided here once ([`hot_desks`]): zirc-chat reads it
//! for the apps, and zirc.dev through zirc-media's wasm.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What heads the hot desks.
pub const HEADING: &str = "Hot desks";

/// Whose a hot desk's session is when it's the person's own.
pub const YOURS: &str = "Yours";

/// What a screen reader says a hot desk's badge is, after its session's name.
pub const LABEL: &str = "a session at a hot desk";

/// How long a session is still shown at its hot desk once no watch of its is open, in
/// seconds: 5 minutes (the user's choice, 27 Nov 2026). Most watches restart within seconds.
pub const AWAY_SHOWN_SECS: u64 = 5 * 60;

/// The line the hot desks no longer shown fold under, `{n}` how many: `29 away`.
pub const AWAY: &str = "{n} away";

/// Between the parts of a badge's line: `Henrik's · Claude Code`.
const SEPARATOR: &str = " · ";

/// Whether a session is in the office at `now` (seconds since the epoch): a watch of its is
/// open (`online`), or its last closed (`away_since`) less than [`AWAY_SHOWN_SECS`] ago. One
/// away since no one knows when is not.
#[must_use]
pub fn in_office(online: bool, away_since: Option<u64>, now: u64) -> bool {
    online || away_since.is_some_and(|since| now.saturating_sub(since) < AWAY_SHOWN_SECS)
}

/// The line `n` folded hot desks are under: `29 away` ([`AWAY`]), `1 away`.
#[must_use]
pub fn away(n: usize) -> String {
    AWAY.replace("{n}", &n.to_string())
}

/// One session at a hot desk, as the reader knows it ([`hot_desks`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "HotDeskSeen", optional_fields)]
pub struct Seen {
    /// What the reader finds it by again, given back on its badge: its conversation,
    /// `henrik-mbp/cc-1`.
    pub key: String,
    /// The session's id, `cc-1`: what its character is dealt by.
    pub session: String,
    /// The name it goes by: `checkout-form`, or its id.
    pub name: String,
    /// Whose it is, by the name they're shown by: `Henrik`. None when unknown, or with
    /// [`Seen::mine`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Whether it's the person's own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mine: bool,
    /// Its agent tool, as people read it: `Claude Code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// What it's doing: `Changing the payment form`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doing: Option<String>,
    /// Whether a watch of its is open.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub online: bool,
    /// When its last watch closed, in seconds since the epoch, while it's away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub away_since: Option<u64>,
}

/// What [`hot_desks`] is given: by zirc-chat, and by zirc.dev through zirc-media's wasm.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "HotDeskFacts")]
pub struct Facts {
    /// Each session at a hot desk.
    pub seen: Vec<Seen>,
    /// The characters the office's coworkers have, which no hot desk is dealt.
    #[serde(default)]
    pub taken: Vec<String>,
    /// Now, in seconds since the epoch.
    pub now: u64,
}

/// A hot desk's badge: dashed, [`crate::coworkers::HOT_DESK`] atop it, its name in mono,
/// [`Badge::whose`] under it and [`Badge::doing`] in the strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "HotDeskBadge", optional_fields)]
pub struct Badge {
    /// [`Seen::key`], as given.
    pub key: String,
    /// The name it goes by, in mono: `checkout-form`.
    pub name: String,
    /// Its dealt character ([`crate::characters::deal`]), dimmed on the muted ground.
    pub character: String,
    /// Whose it is and its tool: `Henrik's · Claude Code`, `Yours · Codex`.
    pub whose: String,
    /// What it's doing, in the strip. None while nothing's said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doing: Option<String>,
    /// What a screen reader says of it: `checkout-form, a session at a hot desk, Henrik's ·
    /// Claude Code, Changing the payment form`.
    pub label: String,
    /// Whether a watch of its is open: its dot.
    pub online: bool,
}

/// The hot desks, split into who's in the office and who's folded away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "HotDesks", optional_fields)]
pub struct HotDesks {
    /// What heads them: [`HEADING`].
    pub heading: String,
    /// Those in the office ([`in_office`]), online first, then by name.
    pub shown: Vec<Badge>,
    /// The line the rest fold under, `29 away` ([`away`]); None while none are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub away: Option<String>,
    /// The rest, by name, shown once [`HotDesks::away`] is opened.
    pub folded: Vec<Badge>,
}

/// Whose `seen` is and its tool: `Henrik's · Claude Code`, `Yours`, or nothing known.
fn whose(seen: &Seen) -> String {
    let owner = if seen.mine {
        Some(YOURS.to_owned())
    } else {
        seen.owner.as_deref().map(crate::possessive)
    };
    [owner, seen.tool.clone()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(SEPARATOR)
}

/// `seen`'s badge, its character dealt from those not `taken`.
fn badge(seen: &Seen, taken: &[String]) -> Badge {
    let whose = whose(seen);
    let label = [
        Some(format!("{}, {LABEL}", seen.name)),
        (!whose.is_empty()).then(|| whose.clone()),
        seen.doing.clone(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(", ");
    Badge {
        key: seen.key.clone(),
        name: seen.name.clone(),
        character: crate::characters::deal(&seen.session, taken.iter().map(String::as_str))
            .to_owned(),
        whose,
        doing: seen.doing.clone(),
        label,
        online: seen.online,
    }
}

/// The hot desks of the sessions `facts` says were seen, at its `now`.
#[must_use]
pub fn hot_desks(facts: &Facts) -> HotDesks {
    let (mut shown, mut folded): (Vec<&Seen>, Vec<&Seen>) = facts
        .seen
        .iter()
        .partition(|s| in_office(s.online, s.away_since, facts.now));
    shown.sort_by(|a, b| b.online.cmp(&a.online).then_with(|| a.name.cmp(&b.name)));
    folded.sort_by(|a, b| a.name.cmp(&b.name));
    HotDesks {
        heading: HEADING.to_owned(),
        away: (!folded.is_empty()).then(|| away(folded.len())),
        shown: shown.into_iter().map(|s| badge(s, &facts.taken)).collect(),
        folded: folded.into_iter().map(|s| badge(s, &facts.taken)).collect(),
    }
}

/// The TypeScript zirc.dev reads hot desks as.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        Seen::decl(config),
        Facts::decl(config),
        Badge::decl(config),
        HotDesks::decl(config),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(name: &str, online: bool, away_since: Option<u64>) -> Seen {
        Seen {
            key: format!("m/{name}"),
            session: format!("cc-{name}"),
            name: name.to_owned(),
            online,
            away_since,
            ..Seen::default()
        }
    }

    #[test]
    fn a_session_is_in_the_office_while_online_and_five_minutes_after() {
        let now = 10_000;
        assert!(in_office(true, None, now));
        assert!(in_office(false, Some(now - 60), now));
        assert!(in_office(false, Some(now - AWAY_SHOWN_SECS + 1), now));
        assert!(!in_office(false, Some(now - AWAY_SHOWN_SECS), now));
        assert!(!in_office(false, None, now));
    }

    #[test]
    fn those_away_longer_fold_under_one_line() {
        let now = 100_000;
        let desks = hot_desks(&Facts {
            seen: vec![
                seen("zeta", false, Some(now - 3600)),
                seen("beta", false, Some(now - 30)),
                seen("gamma", true, None),
                seen("alpha", false, Some(now - 50_000)),
                seen("delta", false, None),
            ],
            taken: Vec::new(),
            now,
        });
        let names = |badges: &[Badge]| badges.iter().map(|b| b.name.clone()).collect::<Vec<_>>();
        assert_eq!(desks.heading, "Hot desks");
        assert_eq!(names(&desks.shown), ["gamma", "beta"]);
        assert_eq!(names(&desks.folded), ["alpha", "delta", "zeta"]);
        assert_eq!(desks.away.as_deref(), Some("3 away"));
        assert_eq!(away(1), "1 away");
        let one = Facts {
            seen: vec![seen("a", true, None)],
            taken: Vec::new(),
            now,
        };
        assert_eq!(hot_desks(&one).away, None);
    }

    #[test]
    fn a_badge_says_whose_it_is_its_tool_and_what_its_doing() {
        let henriks = Seen {
            owner: Some("Henrik".to_owned()),
            tool: Some("Claude Code".to_owned()),
            doing: Some("Changing the payment form".to_owned()),
            ..seen("checkout-form", true, None)
        };
        let badge = badge(&henriks, &[]);
        // Dealt as every app deals a session, never a character a coworker has.
        assert_eq!(
            badge.character,
            crate::characters::deal("cc-checkout-form", [])
        );
        let taken = [badge.character.clone()];
        assert_ne!(super::badge(&henriks, &taken).character, badge.character);
        assert_eq!(badge.whose, "Henrik's · Claude Code");
        assert_eq!(
            badge.label,
            "checkout-form, a session at a hot desk, Henrik's · Claude Code, Changing the payment form"
        );
        let mine = super::badge(
            &Seen {
                mine: true,
                ..seen("tls", false, Some(1))
            },
            &[],
        );
        assert_eq!(mine.whose, "Yours");
        assert_eq!(mine.label, "tls, a session at a hot desk, Yours");
    }
}
