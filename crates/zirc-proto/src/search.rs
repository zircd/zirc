//! A search as people type it, for every app, the TUI and the CLI alike: words, plus tokens
//! that narrow it (`in:#api from:leo agent:checkout type:landing has:link after:2026-11-01
//! before:2026-11-07 sort:best`). [`Query::parse`] reads one, [`Query`]'s `Display` writes it
//! back, so a recent search or a chip's removal round-trips; [`Query::params`] is what to ask
//! the server's history for ([`zirc_proto::login::HISTORY_PATH`]).
//!
//! Where it looks is its [`Scope`]: one conversation, or everything on the server the person
//! may read. Opened inside a conversation it starts there; anywhere else, everywhere
//! (`search-scope-ux.md`, the design lead's note). `in:*` is everywhere on the server, `in:**`
//! every server (later).
//!
//! It lives here, beside the history's wire types, so the CLI reads a search as the apps do
//! (`zirc-chat` re-exports it as `SearchQuery`).

use std::fmt;

/// Where a search looks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// One conversation: `#channel`, or `@nick` for direct messages with them.
    Conversation(String),
    /// One agent's session on a machine: `<machine>/<session>`.
    Agent {
        /// The machine's nick.
        machine: String,
        /// The agent's session.
        session: String,
    },
    /// Everything on the server the person may read.
    Server,
    /// Every server the person is signed in to (later).
    AllServers,
}

/// What order results come in: as typed after `sort:`, which is how the web chat's JSON
/// says it too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum SearchSort {
    /// The best matches first.
    Best,
    /// The newest first.
    New,
    /// The oldest first.
    Old,
}

impl SearchSort {
    /// How it's typed: `best`, `new` or `old`.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Best => "best",
            Self::New => "new",
            Self::Old => "old",
        }
    }

    /// The order typed as `token`, as [`SearchSort::token`] writes it.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        [Self::Best, Self::New, Self::Old]
            .into_iter()
            .find(|sort| sort.token() == token)
    }
}

/// A search, as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    /// The words to find, as typed, quotes included.
    pub text: String,
    /// Where to look.
    pub scope: Scope,
    /// The order asked for, if one was; see [`Query::sort`].
    pub sort: Option<SearchSort>,
    /// Said by this nick or account.
    pub from: Option<String>,
    /// Said by this agent.
    pub agent: Option<String>,
    /// Kinds of message, such as `landing` or `event`.
    pub kinds: Vec<String>,
    /// What messages have, such as `link`, `file` or `code`.
    pub has: Vec<String>,
    /// On or after this day, `YYYY-MM-DD`.
    pub after: Option<String>,
    /// Before this day, `YYYY-MM-DD`.
    pub before: Option<String>,
}

/// A part of a [`Query`] shown on its own, which can be taken away: a chip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip {
    /// What it shows, such as `in #api` or `from leo`.
    pub label: String,
    /// The token it stands for, such as `in:#api`.
    pub token: String,
}

/// Why typed text isn't a [`Query`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable(pub String);

impl fmt::Display for Unreadable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Query {
    /// The search `text` types, opened in the conversation `opened_in` (`#channel`, `@nick`)
    /// or elsewhere (`None`), which is where it looks unless it says `in:`.
    ///
    /// # Errors
    ///
    /// A token whose value can't be read, such as a date that isn't `YYYY-MM-DD`.
    pub fn parse(text: &str, opened_in: Option<&str>) -> Result<Self, Unreadable> {
        let mut query = Self {
            text: String::new(),
            scope: opened_in.map_or(Scope::Server, scope_of),
            sort: None,
            from: None,
            agent: None,
            kinds: Vec::new(),
            has: Vec::new(),
            after: None,
            before: None,
        };
        let mut words = Vec::new();
        for token in tokens(text) {
            let Some((key, value)) = token
                .split_once(':')
                .filter(|(k, v)| !v.is_empty() && !token.starts_with('"') && KEYS.contains(k))
            else {
                words.push(token);
                continue;
            };
            match key {
                "in" => query.scope = scope_of(value),
                "from" => query.from = Some(value.trim_start_matches('@').to_owned()),
                "agent" => query.agent = Some(value.to_owned()),
                "type" => query.kinds.push(value.to_owned()),
                "has" => query.has.push(value.to_owned()),
                "after" => query.after = Some(day(value)?),
                "before" => query.before = Some(day(value)?),
                "sort" => {
                    let Some(sort) = SearchSort::from_token(value) else {
                        return Err(Unreadable(format!("sort is best, new or old, not {value}")));
                    };
                    query.sort = Some(sort);
                }
                _ => words.push(token),
            }
        }
        query.text = words.join(" ");
        Ok(query)
    }

    /// The order results come in: the one asked for, else the best first everywhere and the
    /// newest first in one conversation.
    #[must_use]
    pub fn sort(&self) -> SearchSort {
        self.sort.unwrap_or(match self.scope {
            Scope::Server | Scope::AllServers => SearchSort::Best,
            Scope::Conversation(_) | Scope::Agent { .. } => SearchSort::New,
        })
    }

    /// The parts shown as chips, scope first, each with the token it takes away.
    #[must_use]
    pub fn chips(&self) -> Vec<Chip> {
        let mut chips = Vec::new();
        let mut chip = |label: String, token: String| chips.push(Chip { label, token });
        match &self.scope {
            Scope::Conversation(name) => chip(format!("in {name}"), format!("in:{name}")),
            Scope::Agent { machine, session } => chip(
                format!("in {machine}/{session}"),
                format!("in:{machine}/{session}"),
            ),
            Scope::Server | Scope::AllServers => {}
        }
        if let Some(from) = &self.from {
            chip(format!("from {from}"), format!("from:{from}"));
        }
        if let Some(agent) = &self.agent {
            chip(format!("agent {agent}"), format!("agent:{agent}"));
        }
        for kind in &self.kinds {
            chip(kind.clone(), format!("type:{kind}"));
        }
        for has in &self.has {
            chip(format!("has {has}"), format!("has:{has}"));
        }
        if let Some(after) = &self.after {
            chip(format!("after {after}"), format!("after:{after}"));
        }
        if let Some(before) = &self.before {
            chip(format!("before {before}"), format!("before:{before}"));
        }
        chips
    }

    /// The query without `chip`; without its scope chip it looks everywhere.
    #[must_use]
    pub fn without(&self, chip: &Chip) -> Self {
        let mut query = self.clone();
        let (key, value) = chip.token.split_once(':').unwrap_or((&chip.token, ""));
        match key {
            "in" => query.scope = Scope::Server,
            "from" => query.from = None,
            "agent" => query.agent = None,
            "type" => query.kinds.retain(|k| k != value),
            "has" => query.has.retain(|h| h != value),
            "after" => query.after = None,
            "before" => query.before = None,
            _ => {}
        }
        query
    }

    /// The same search everywhere on the server.
    #[must_use]
    pub fn widened(&self) -> Self {
        Self {
            scope: Scope::Server,
            ..self.clone()
        }
    }

    /// What to ask the server's history for: its query's pairs but the cursor and limit.
    /// Counts are asked for always: inside one conversation they say how many matches there
    /// are elsewhere, in one answer.
    #[must_use]
    pub fn params(&self) -> Vec<(&'static str, String)> {
        let mut params = Vec::new();
        if !self.text.is_empty() {
            params.push(("q", self.text.trim_matches('"').to_owned()));
        }
        if let Scope::Conversation(name) = &self.scope {
            params.push(("conversation", name.clone()));
        }
        if let Some(from) = &self.from {
            params.push(("sender", from.clone()));
        }
        if self.kinds.iter().any(|k| k != "message") {
            params.push(("kinds", "all".to_owned()));
        }
        if let Some(after) = &self.after {
            params.push(("since", format!("{after}T00:00:00Z")));
        }
        if let Some(before) = &self.before {
            params.push(("until", format!("{before}T00:00:00Z")));
        }
        params.push((
            "sort",
            match self.sort() {
                SearchSort::Best => "best",
                SearchSort::New | SearchSort::Old => "new",
            }
            .to_owned(),
        ));
        params.push(("counts", "1".to_owned()));
        params
    }
}

impl fmt::Display for Query {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        parts.push(match &self.scope {
            Scope::Conversation(name) => format!("in:{name}"),
            Scope::Agent { machine, session } => format!("in:{machine}/{session}"),
            Scope::Server => "in:*".to_owned(),
            Scope::AllServers => "in:**".to_owned(),
        });
        parts.extend(
            self.chips()
                .into_iter()
                .skip(usize::from(matches!(
                    self.scope,
                    Scope::Conversation(_) | Scope::Agent { .. }
                )))
                .map(|c| c.token),
        );
        if let Some(sort) = self.sort {
            parts.push(format!("sort:{}", sort.token()));
        }
        if !self.text.is_empty() {
            parts.push(self.text.clone());
        }
        f.write_str(&parts.join(" "))
    }
}

/// The keys a token may have.
const KEYS: [&str; 8] = [
    "in", "from", "agent", "type", "has", "after", "before", "sort",
];

/// Where `in:<value>` looks.
fn scope_of(value: &str) -> Scope {
    match value {
        "*" => Scope::Server,
        "**" => Scope::AllServers,
        _ => match value.split_once('/') {
            Some((machine, session)) if !value.starts_with('#') => Scope::Agent {
                machine: machine.to_owned(),
                session: session.to_owned(),
            },
            _ => Scope::Conversation(value.to_lowercase()),
        },
    }
}

/// `value` if it's a day, `YYYY-MM-DD`, with a month and a day of the month that can be.
fn day(value: &str) -> Result<String, Unreadable> {
    let unreadable = || Unreadable(format!("a day is YYYY-MM-DD, not {value}"));
    let mut parts = value.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(unreadable());
    };
    let number = |text: &str, digits: usize| {
        (text.len() == digits && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse::<u32>().ok())
            .flatten()
    };
    let (Some(year), Some(month), Some(day)) = (number(year, 4), number(month, 2), number(day, 2))
    else {
        return Err(unreadable());
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return Err(unreadable()),
    };
    if day == 0 || day > days {
        return Err(unreadable());
    }
    Ok(value.to_owned())
}

/// The words of `text`, a quoted phrase as one, quotes kept.
fn tokens(text: &str) -> Vec<String> {
    let (mut tokens, mut current, mut quoted) = (Vec::new(), String::new(), false);
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_starts_where_it_was_opened_and_in_says_otherwise() {
        let here = Query::parse("timeout", Some("#API")).unwrap();
        assert_eq!(here.scope, Scope::Conversation("#api".into()));
        assert_eq!(here.sort(), SearchSort::New);
        let home = Query::parse("timeout", None).unwrap();
        assert_eq!(home.scope, Scope::Server);
        assert_eq!(home.sort(), SearchSort::Best);
        let everywhere = Query::parse("in:* timeout", Some("#api")).unwrap();
        assert_eq!(everywhere.scope, Scope::Server);
        assert_eq!(
            Query::parse("in:** x", None).unwrap().scope,
            Scope::AllServers
        );
        let agent = Query::parse("in:mattias-mbp/cc-1a2b3c tests", None).unwrap();
        assert_eq!(
            agent.scope,
            Scope::Agent {
                machine: "mattias-mbp".into(),
                session: "cc-1a2b3c".into()
            }
        );
        assert_eq!(
            Query::parse("in:@leo x", None).unwrap().scope,
            Scope::Conversation("@leo".into())
        );
    }

    #[test]
    fn tokens_narrow_and_the_rest_is_text_quotes_and_unknown_keys_included() {
        let query = Query::parse(
            r#"from:@leo agent:checkout type:landing has:link after:2026-11-01 "in:the box" https://x.dev/a sort:old"#,
            None,
        )
        .unwrap();
        assert_eq!(query.from.as_deref(), Some("leo"));
        assert_eq!(query.agent.as_deref(), Some("checkout"));
        assert_eq!(query.kinds, ["landing"]);
        assert_eq!(query.has, ["link"]);
        assert_eq!(query.after.as_deref(), Some("2026-11-01"));
        assert_eq!(query.sort(), SearchSort::Old);
        assert_eq!(query.text, r#""in:the box" https://x.dev/a"#);
        assert!(Query::parse("after:yesterday", None).is_err());
        assert!(Query::parse("after:2026-02-30", None).is_err());
        assert!(Query::parse("before:2028-02-29", None).is_ok());
        assert!(Query::parse("sort:random", None).is_err());
    }

    #[test]
    fn it_round_trips_through_its_text() {
        for (typed, opened_in) in [
            ("in:#api from:leo timeout", None),
            ("timeout", Some("#api")),
            (
                "type:landing has:file before:2026-11-07 sort:best \"gateway down\"",
                None,
            ),
            ("in:mattias-mbp/cc-1 tests", Some("#web")),
        ] {
            let query = Query::parse(typed, opened_in).unwrap();
            let again = Query::parse(&query.to_string(), Some("#elsewhere")).unwrap();
            assert_eq!(again, query, "{typed} → {query}");
        }
    }

    #[test]
    fn chips_come_away_one_by_one_and_widening_looks_everywhere() {
        let query = Query::parse("from:leo has:link timeout", Some("#api")).unwrap();
        let chips = query.chips();
        let labels: Vec<_> = chips.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["in #api", "from leo", "has link"]);
        let wider = query.without(&chips[0]);
        assert_eq!(wider.scope, Scope::Server);
        assert_eq!(wider.from.as_deref(), Some("leo"));
        assert_eq!(query.without(&chips[2]).has, Vec::<String>::new());
        assert_eq!(query.widened().scope, Scope::Server);
        assert_eq!(query.widened().text, "timeout");
    }

    #[test]
    fn it_asks_the_server_with_counts_and_its_sort() {
        let here = Query::parse("from:leo \"gateway down\"", Some("#api")).unwrap();
        let params = here.params();
        let get = |key: &str| {
            params
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("q"), Some("gateway down"));
        assert_eq!(get("conversation"), Some("#api"));
        assert_eq!(get("sender"), Some("leo"));
        assert_eq!(get("sort"), Some("new"));
        assert_eq!(get("counts"), Some("1"));
        let everywhere = Query::parse("type:event x", None).unwrap();
        let params = everywhere.params();
        assert!(params.contains(&("sort", "best".to_owned())));
        assert!(params.contains(&("kinds", "all".to_owned())));
        assert!(!params.iter().any(|(k, _)| *k == "conversation"));
    }
}
