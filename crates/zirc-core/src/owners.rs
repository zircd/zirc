//! Who works on a path: the sessions changing it now or about to ([`crate::clashes`]), and
//! those whose commits and pushes changed it lately, from a channel's history. What
//! `zirc who --owns` says, so a session asks the one that knows a part of the code, rather
//! than whoever it last heard from.

use std::collections::BTreeMap;

use zirc_proto::Event;

use crate::clashes::{self, Clash, What};
use crate::ipc::{ChannelPresence, Delivery};
use crate::presence::touching;

/// A session that works on the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    /// Who, as [`clashes::label`] calls them.
    pub who: String,
    /// What it's doing there now: changing it, or about to.
    pub now: Vec<Clash>,
    /// How many of its commits and pushes in the history read changed the path.
    pub changed: usize,
    /// Whom `zirc ask --to` reaches it as ([`clashes::ask_target`]), when it says which
    /// session.
    pub ask: Option<String>,
}

/// Who works on `path` in `channel`: those on it now first, then by how often they changed
/// it in `history`, most first.
#[must_use]
pub fn owners(channel: &ChannelPresence, history: &[Delivery], path: &str, now: u64) -> Vec<Owner> {
    owners_of(channel, history, &[path.to_owned()], "", now)
}

/// Who works on any of `paths` in `channel`, but `me`, a session, as [`owners`] orders them.
#[must_use]
pub fn owners_of(
    channel: &ChannelPresence,
    history: &[Delivery],
    paths: &[String],
    me: &str,
    now: u64,
) -> Vec<Owner> {
    let paths: Vec<String> = paths
        .iter()
        .map(|path| {
            path.trim_start_matches("./")
                .trim_end_matches('/')
                .to_owned()
        })
        .collect();
    let mine: Vec<&str> = paths.iter().map(String::as_str).collect();
    let mut by: BTreeMap<String, Owner> = BTreeMap::new();
    for clash in clashes::clashes(channel, me, &paths, now) {
        let owner = by.entry(clash.who.clone()).or_insert_with(|| Owner {
            who: clash.who.clone(),
            now: Vec::new(),
            changed: 0,
            ask: None,
        });
        owner.ask = owner.ask.take().or_else(|| clash.ask.clone());
        owner.now.push(clash);
    }
    for delivery in history {
        let message = &delivery.message;
        let (Event::Commit { paths: changed, .. } | Event::Push { paths: changed, .. }) =
            &message.event
        else {
            continue;
        };
        if touching(changed, &mine).is_empty() {
            continue;
        }
        let session = (!message.session.is_empty()).then_some(message.session.as_str());
        if !me.is_empty() && session == Some(me) {
            continue;
        }
        let who = clashes::label(&delivery.from, session, message.name.as_deref());
        let owner = by.entry(who.clone()).or_insert_with(|| Owner {
            who,
            now: Vec::new(),
            changed: 0,
            ask: None,
        });
        owner.changed += 1;
        owner.ask = owner
            .ask
            .take()
            .or_else(|| clashes::ask_target(&delivery.from, session));
    }
    let mut owners: Vec<Owner> = by.into_values().collect();
    owners.sort_by(|a, b| {
        let key = |owner: &Owner| (owner.now.is_empty(), std::cmp::Reverse(owner.changed));
        key(a).cmp(&key(b)).then_with(|| a.who.cmp(&b.who))
    });
    owners
}

/// `owner` in a line: `cc-1 (desktop): changing it on desk-x; about to change it: the
/// header; 6 commits there lately; ask it with --to mbp/cc-1`.
#[must_use]
pub fn line(owner: &Owner) -> String {
    let mut parts: Vec<String> = owner
        .now
        .iter()
        .map(|clash| match (&clash.what, &clash.branch) {
            (What::Changing, Some(branch)) => format!("changing it on {branch}"),
            (What::Changing, None) => "changing it".to_owned(),
            (What::Intends(summary), _) => format!("about to change it: {summary}"),
        })
        .collect();
    match owner.changed {
        0 => {}
        1 => parts.push("1 commit there lately".to_owned()),
        n => parts.push(format!("{n} commits there lately")),
    }
    if let Some(ask) = &owner.ask {
        parts.push(format!("ask it with --to {ask}"));
    }
    format!("{}: {}", owner.who, parts.join("; "))
}

/// Forgets whom to ask for each of `owners` on this machine, one of `nicks`, whose session
/// has no watch running (`watching`, as [`crate::ipc::Status::watching`] lists them): a
/// question to it would wait for nothing. Those on other machines are kept, as this one
/// can't tell.
pub fn forget_gone(owners: &mut [Owner], nicks: &[&str], watching: &[Option<String>]) {
    for owner in owners {
        let gone = owner
            .ask
            .as_deref()
            .and_then(|ask| ask.split_once('/'))
            .is_some_and(|(nick, session)| {
                nicks.contains(&nick) && !watching.iter().flatten().any(|s| s == session)
            });
        if gone {
            owner.ask = None;
        }
    }
}

/// How many sessions [`ask_text`] names at most.
const ASK_AT_MOST: usize = 3;

/// The sessions that changed the paths lately and aren't on them now, as lines saying to ask
/// them how that code works before reading it all: a session with it in mind answers in
/// seconds what costs another many files' reading. Those on the paths now are for
/// [`clashes::text`] to name. Empty when there are none to ask.
#[must_use]
pub fn ask_text(owners: &[Owner]) -> String {
    use std::fmt::Write as _;
    let askable: Vec<&Owner> = owners
        .iter()
        .filter(|owner| owner.now.is_empty() && owner.changed > 0 && owner.ask.is_some())
        .take(ASK_AT_MOST)
        .collect();
    if askable.is_empty() {
        return String::new();
    }
    let mut text = String::from(
        "changed these lately: before reading code you don't know, ask how it works \
         (`zirc ask --session <you> --to <them> \"<question>\"`), and read it yourself if \
         no answer comes within a few minutes.\n",
    );
    for owner in askable {
        let commits = match owner.changed {
            1 => "1 commit".to_owned(),
            n => format!("{n} commits"),
        };
        let ask = owner.ask.as_deref().unwrap_or_default();
        let _ = writeln!(text, "  {}: {commits}, --to {ask}", owner.who);
    }
    text
}

#[cfg(test)]
mod tests {
    use zirc_proto::Message;

    use super::*;
    use crate::presence::{Intent, Member};

    fn commit(from: &str, session: &str, name: Option<&str>, paths: &[&str]) -> Delivery {
        Delivery {
            channel: "#api".into(),
            from: from.into(),
            message: Message {
                name: name.map(str::to_owned),
                ..Message::new(
                    session,
                    Event::Commit {
                        branch: "work".into(),
                        sha: "abc".into(),
                        subject: "a change".into(),
                        paths: paths.iter().map(|&path| path.to_owned()).collect(),
                        repo: None,
                    },
                )
            },
            id: None,
            time: None,
            why: None,
            reply: None,
            org: None,
            account: None,
            hosted_by: None,
            client_of: None,
            decided: None,
            fit: None,
        }
    }

    #[test]
    fn those_on_it_now_come_first_then_those_who_changed_it_most() {
        let channel = ChannelPresence {
            channel: "#api".into(),
            org: None,
            members: vec![Member {
                nick: "mbp".into(),
                worktree: None,
                name: None,
                branch: Some("desk-x".into()),
                session: Some("cc-1".into()),
                session_name: Some("desktop".into()),
                session_tool: None,
                session_avatar: None,
                paths: vec!["apps/desktop/src/app.rs".into()],
            }],
            claims: Vec::new(),
            intents: vec![Intent {
                nick: "mini".into(),
                session: "cc-3".into(),
                session_name: None,
                session_tool: None,
                session_avatar: None,
                summary: "the header".into(),
                paths: vec!["apps/desktop".into()],
                expires: None,
            }],
            landings: Vec::new(),
            built: Vec::new(),
        };
        let history = [
            commit("mini", "cc-2", Some("chat"), &["crates/chat/src/lib.rs"]),
            commit("mini", "cc-2", Some("chat"), &["apps/desktop/src/app.rs"]),
            commit(
                "mbp",
                "cc-1",
                Some("desktop"),
                &["apps/desktop/core/src/lib.rs"],
            ),
            commit("mini", "cc-2", Some("chat"), &["apps/desktop/src/main.rs"]),
            commit("mini", "cc-4", None, &["apps/desktop/src/main.rs"]),
        ];
        let found: Vec<String> = owners(&channel, &history, "./apps/desktop/", 0)
            .iter()
            .map(line)
            .collect();
        assert_eq!(
            found,
            [
                "cc-1 (desktop): changing it on desk-x; 1 commit there lately; ask it with --to mbp/cc-1",
                "cc-3: about to change it: the header; ask it with --to mini/cc-3",
                "cc-2 (chat): 2 commits there lately; ask it with --to mini/cc-2",
                "cc-4: 1 commit there lately; ask it with --to mini/cc-4",
            ]
        );
        assert!(owners(&channel, &history, "web", 0).is_empty());
        // Asked about by cc-4, about to change apps/desktop/src: cc-1 and cc-3 are on it now,
        // for the heads-up to name, and cc-4 is the one asking.
        let paths = ["apps/desktop/src".to_owned()];
        let asking = owners_of(&channel, &history, &paths, "cc-4", 0);
        assert!(asking.iter().all(|owner| owner.who != "cc-4"));
        assert_eq!(
            ask_text(&asking),
            "changed these lately: before reading code you don't know, ask how it works \
             (`zirc ask --session <you> --to <them> \"<question>\"`), and read it yourself if \
             no answer comes within a few minutes.\n  cc-2 (chat): 2 commits, --to mini/cc-2\n"
        );
        assert_eq!(
            ask_text(&owners_of(&channel, &history, &["web".to_owned()], "", 0)),
            ""
        );
    }
}
