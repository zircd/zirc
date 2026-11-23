//! The stream's decisions in plain text, for an app that draws text alone, the terminal's:
//! the same choices the other apps draw with their own widgets, made once here. What a
//! message's reactions come to, which of a card's buttons show, the pill after a name, a
//! reply's quote, an event's one line and its words, and what a landing says with the links
//! in it. The app lays them out: widths, wrapping, colours and columns stay its own.

use zirc_proto::card::{Action, Card};

use super::conversation::{Conversation, Quote};
use super::view::{Icon, Landing, Parts};
use super::{APP_TAG, Block, SpanKind, each_span};

/// What a list of people heads zirc's apps with, CI and the like, under the people.
pub const APPS_SECTION: &str = "Apps";

/// What the people list is called, over everyone in a conversation, one row each.
pub const PEOPLE_SECTION: &str = "People";

/// What the channel browser says when there's no channel left to join.
pub const NO_OTHER_CHANNELS: &str = "No other channels to join";

/// What marks a post addressed to an agent session, before its name: `→ zirc-chat …`.
pub const TO_MARK: &str = "→";

/// What marks the quote of what a reply answers, above it: `↪ bob: first line…`.
pub const QUOTE_MARK: &str = "↪";

/// One reaction to a message, as every app shows it under the message: `🎉 2`, ours marked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReactionChip {
    /// The emoji.
    pub emoji: String,
    /// How many gave it.
    pub count: usize,
    /// Whether `me` is one of them.
    pub mine: bool,
}

impl Conversation {
    /// The reactions to the message `id`, `me` being our nick, in the order they were first
    /// given: each emoji, how many gave it and whether we did.
    #[must_use]
    pub fn chips(&self, id: &str, me: &str) -> Vec<ReactionChip> {
        self.reactions(id)
            .iter()
            .map(|reaction| ReactionChip {
                emoji: reaction.emoji.clone(),
                count: reaction.nicks.len(),
                mine: reaction.by(me),
            })
            .collect()
    }
}

/// The pill after the name heading a line: an app's [`APP_TAG`] when its speaker is one
/// (`app`, [`super::Speaker::is_app`]); a person and an agent have none.
#[must_use]
pub fn name_pill(app: bool) -> Option<&'static str> {
    app.then_some(APP_TAG)
}

impl super::Chat {
    /// The buttons of `card` we're shown ([`super::Chat::shows_action`]), in order.
    #[must_use]
    pub fn card_buttons<'c>(&self, card: &'c Card) -> Vec<&'c Action> {
        card.actions
            .iter()
            .filter(|action| self.shows_action(card, action))
            .collect()
    }

    /// `parts` of a post addressed to an agent session ([`Parts::to`]) with whom it's
    /// addressed to in front of its details, by the session's name: `→ zirc-chat can you
    /// look?`. Anything else as it is.
    #[must_use]
    pub fn addressed_parts(&self, mut parts: Parts) -> Parts {
        if let Some(to) = parts.to.take() {
            parts.details = format!("{TO_MARK} {} {}", self.session_label(&to), parts.details);
        }
        parts
    }
}

impl Quote {
    /// The quote as one line above the reply: `↪ bob: first line…`.
    #[must_use]
    pub fn line(&self) -> String {
        format!("{QUOTE_MARK} {}: {}", self.name, self.text)
    }
}

impl Parts {
    /// A file two are changing at once, said as one phrase with its paths in it, never cut,
    /// as every app says it ([`super::Chat::both_changing_line`]): `also changing
    /// web/src/api/client.ts with checkout`. `None` for anything else, which says its lead
    /// and details apart.
    #[must_use]
    pub fn phrase(&self) -> Option<String> {
        if self.icon != Icon::Overlap {
            return None;
        }
        let paths = self.paths.join(" ");
        let words: Vec<&str> = [self.verb.as_str(), paths.as_str(), self.details.as_str()]
            .into_iter()
            .filter(|word| !word.is_empty())
            .collect();
        Some(words.join(" "))
    }

    /// What its body says under its lead: its details; or, for an event zirc-chat has no
    /// verb or details for (zircd's lease and rename lines), the item's own words, `text`,
    /// rather than a lead over nothing.
    #[must_use]
    pub fn words(&self, text: &str) -> String {
        if self.verb.is_empty() && self.details.is_empty() {
            text.to_owned()
        } else {
            self.details.clone()
        }
    }

    /// It as one line, as IRC's display mode and people coming and going show it: who, what
    /// they did, then what it was, `dana's agent committed: Fix login`; or who and its
    /// [`Parts::phrase`]. Its paths aren't in it, but in the phrase.
    #[must_use]
    pub fn line(&self) -> String {
        let phrase = self.phrase();
        let did = [self.actor.as_str(), phrase.as_deref().unwrap_or(&self.verb)]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        match (did.is_empty(), self.details.is_empty() || phrase.is_some()) {
            (_, true) => did,
            (true, false) => self.details.clone(),
            (false, false) => format!("{did}: {}", self.details),
        }
    }
}

impl Landing<'_> {
    /// What it says as plain text, line by line: a card's title and state, then its fields,
    /// links and note as [`Landing::rich`] lays them out; a landing's line, `fix-login ·
    /// build 771 · ✓ landed · ✓ production updated` with its state's mark, and under it
    /// what CI's latest event says below its first line; or what a post of its own said.
    #[must_use]
    pub fn plain(&self) -> String {
        if self.card.is_some() {
            return self
                .rich()
                .iter()
                .map(block_text)
                .collect::<Vec<_>>()
                .join("\n");
        }
        let Some(line) = self.card_line() else {
            return self.said().to_owned();
        };
        let line: String = line.into_iter().map(|span| span.text).collect();
        match self.said().split_once('\n') {
            Some((_, details)) if !details.trim().is_empty() => {
                format!("{line}\n{}", details.trim())
            }
            _ => line,
        }
    }

    /// The words of [`Landing::plain`] that link somewhere, each with its address, once
    /// each: a card's links; a landing's build, then its failed targets, commit and branch
    /// ([`Landing::links`]).
    #[must_use]
    pub fn plain_links(&self) -> Vec<(String, String)> {
        let mut linked = Vec::new();
        if self.card.is_some() {
            each_span(&self.rich(), &mut |span| {
                if let SpanKind::Link(url) = &span.kind {
                    linked.push((span.text.clone(), url.clone()));
                }
            });
            return linked;
        }
        linked.extend(
            self.card_line()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|span| match span.kind {
                    SpanKind::Link(url) => Some((span.text, url)),
                    _ => None,
                }),
        );
        linked.extend(self.links());
        let mut seen = std::collections::HashSet::new();
        linked.retain(|(word, _)| seen.insert(word.clone()));
        linked
    }
}

/// A block's words on one line: its spans' text, its quotes', lists' and tables' in order,
/// a code block's text.
fn block_text(block: &Block) -> String {
    let mut line = String::new();
    each_span(std::slice::from_ref(block), &mut |span| {
        line.push_str(&span.text);
    });
    line
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone as _, Utc};

    use super::*;

    fn parts(icon: Icon, actor: &str, verb: &str, details: &str, paths: &[&str]) -> Parts {
        let time = Utc.with_ymd_and_hms(2026, 10, 30, 9, 0, 0).unwrap();
        Parts {
            icon,
            actor: actor.into(),
            session: None,
            owner: None,
            actor_detail: None,
            compact: false,
            to: None,
            verb: verb.into(),
            details: details.into(),
            paths: paths.iter().map(|&p| p.to_owned()).collect(),
            links: Vec::new(),
            time,
            until: time,
        }
    }

    #[test]
    fn says_an_event_on_one_line() {
        let commit = parts(Icon::Commit, "dana's agent", "committed", "Fix login", &[]);
        assert_eq!(commit.line(), "dana's agent committed: Fix login");
        assert_eq!(commit.phrase(), None);
        let joined = parts(Icon::Join, "", "alice joined", "", &[]);
        assert_eq!(joined.line(), "alice joined");
        let bare = parts(Icon::Post, "", "", "just words", &[]);
        assert_eq!(bare.line(), "just words");
    }

    #[test]
    fn briefs_an_agents_routine_work_on_one_line() {
        let intent = parts(
            Icon::Intent,
            "checkout",
            "is about to change",
            "fix the coupon field's layout shift",
            &["web/src/checkout/Coupon.tsx"],
        );
        assert_eq!(
            intent.brief().as_deref(),
            Some("checkout is about to change Coupon.tsx")
        );
        let many = parts(
            Icon::Intent,
            "checkout",
            "is about to change",
            "",
            &["a.rs", "web/", "c/d.rs"],
        );
        assert_eq!(
            many.brief().as_deref(),
            Some("checkout is about to change a.rs and 2 more")
        );
        let commit = parts(
            Icon::Commit,
            "checkout",
            "committed",
            "Fix login\n\nmore",
            &[],
        );
        assert_eq!(
            commit.brief().as_deref(),
            Some("checkout committed: Fix login")
        );
        // What asks something of its reader, talk, and what's one line already keep their form.
        for icon in [
            Icon::Post,
            Icon::Landed,
            Icon::Failed,
            Icon::Conflict,
            Icon::Overlap,
        ] {
            assert_eq!(
                parts(icon, "cc-1", "did", "it", &["a.rs"]).brief(),
                None,
                "{icon:?}"
            );
        }
        let joined = Parts {
            compact: true,
            ..parts(Icon::Join, "", "alice joined", "", &[])
        };
        assert_eq!(joined.brief(), None);
    }

    #[test]
    fn says_two_changing_a_file_as_one_phrase_with_every_path() {
        let one = parts(
            Icon::Overlap,
            "cc-1",
            "also changing",
            "with checkout",
            &["web/src/api/client.ts"],
        );
        assert_eq!(
            one.phrase().as_deref(),
            Some("also changing web/src/api/client.ts with checkout")
        );
        assert_eq!(
            one.line(),
            "cc-1 also changing web/src/api/client.ts with checkout"
        );
        let two = parts(Icon::Overlap, "", "also changing", "", &["a.rs", "b.rs"]);
        assert_eq!(two.phrase().as_deref(), Some("also changing a.rs b.rs"));
    }

    #[test]
    fn an_event_without_words_of_its_own_says_its_text() {
        let lease = parts(Icon::Post, "zircd", "", "", &[]);
        assert_eq!(lease.words("lease build is yours"), "lease build is yours");
        let commit = parts(Icon::Commit, "", "committed", "Fix", &[]);
        assert_eq!(commit.words("ignored"), "Fix");
    }

    #[test]
    fn quotes_a_reply_on_one_line() {
        let quote = Quote {
            id: "m1".into(),
            from: "bob".into(),
            name: "Bob Lee".into(),
            text: "first line…".into(),
            avatar: super::super::Avatar::Person { nick: "bob".into() },
        };
        assert_eq!(quote.line(), "↪ Bob Lee: first line…");
    }

    fn chat() -> super::super::Chat {
        let mut chat = super::super::Chat::new(
            "me",
            "irc.zirc.test",
            Vec::new(),
            super::super::Options::default(),
        );
        let _ = chat.connected(super::super::Link {
            nick: "me".into(),
            metadata: false,
            chathistory: false,
            echo: true,
            read_marker: false,
            trim: true,
            multiline: None,
        });
        chat
    }

    #[test]
    fn counts_reactions_in_the_order_given_ours_marked() {
        let mut chat = chat();
        for line in [
            ":me!u@h JOIN #api",
            "@msgid=m1;time=2026-10-30T09:00:00.000Z :alice!u@h PRIVMSG #api :shipped it",
            "@+draft/react=🎉;+draft/reply=m1 :bob!u@h TAGMSG #api",
            "@+draft/react=🎉;+draft/reply=m1 :me!u@h TAGMSG #api",
            "@+draft/react=🚀;+draft/reply=m1 :bob!u@h TAGMSG #api",
        ] {
            let _ = chat.receive(line.parse().unwrap());
        }
        let api = chat.conversation("#api").unwrap();
        assert_eq!(
            api.chips("m1", "me"),
            [
                ReactionChip {
                    emoji: "🎉".into(),
                    count: 2,
                    mine: true
                },
                ReactionChip {
                    emoji: "🚀".into(),
                    count: 1,
                    mine: false
                },
            ]
        );
        assert!(api.chips("nope", "me").is_empty());
    }

    #[test]
    fn says_whom_a_post_is_addressed_to_first() {
        let chat = chat();
        let mut post = parts(Icon::Post, "", "", "can you look?", &[]);
        post.to = Some("cc-598aa1".into());
        let post = chat.addressed_parts(post);
        assert_eq!(post.details, "→ cc-598aa1 can you look?");
        assert_eq!(post.to, None);
    }

    #[test]
    fn says_a_landing_in_plain_text_with_its_links_once_each() {
        use super::super::{Item, Kind};
        let chat = chat();
        let mut zirc = Conversation::new("#zirc", Kind::Channel);
        let text = "cc-cdbba7: landed web-look as 3d5fe8a (build 435)";
        zirc.entries.push(super::super::Entry {
            from: "zirc-ci".into(),
            owner: None,
            kind: super::super::EntryKind::Zirc(zirc_proto::Message::new(
                "ci",
                zirc_proto::Event::Custom {
                    kind: zirc_proto::ci::LANDING_KIND.into(),
                    data: serde_json::Value::default(),
                    text: text.into(),
                },
            )),
            ..super::super::Entry::info(
                Utc.with_ymd_and_hms(2026, 10, 30, 9, 0, 0).unwrap(),
                String::new(),
            )
        });
        let items = chat.items(&zirc, |time| time.date_naive());
        let Some(Item::Landing(landing)) = items.iter().find(|i| matches!(i, Item::Landing(_)))
        else {
            panic!("{items:?}")
        };
        assert_eq!(landing.plain(), "landed web-look as 3d5fe8a (build 435)");
        let links = landing.plain_links();
        let words: Vec<&str> = links.iter().map(|(w, _)| w.as_str()).collect();
        let mut once = words.clone();
        once.dedup();
        assert_eq!(words, once);
    }

    #[test]
    fn names_the_apps_part_and_the_browser_as_every_app_does() {
        assert_eq!(APPS_SECTION, "Apps");
        assert_eq!(NO_OTHER_CHANNELS, "No other channels to join");
    }
}
