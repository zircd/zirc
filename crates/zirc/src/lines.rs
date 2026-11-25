//! How what zircd hears reads as one line.

use std::collections::BTreeMap;

use zirc_core::decisions;
use zirc_core::hunks;
use zirc_core::ipc::{self, Delivery, Direct, LocalOverlap, Overlap, Worktree};
use zirc_proto::decision::Decision;

use crate::client::{channel_in, shown};
use crate::sessions::some_of;

/// A reaction as a line: `#api ada reacted 👍 to 01jbx7ab`, or `took back 👍 on` it, the
/// message by the start of its id, as `zirc react` takes it.
pub(crate) fn reaction_line(reaction: &ipc::Reaction) -> String {
    if reaction.why == Some(ipc::Why::ReactionAwaited) {
        return awaited_reaction_line(reaction);
    }
    let id: String = reaction.to.chars().take(8).collect();
    let verb = if reaction.removed {
        "took back"
    } else {
        "reacted"
    };
    let on = if reaction.removed { "on" } else { "to" };
    format!(
        "{} {} {verb} {} {on} {id}",
        reaction.channel, reaction.from, reaction.emoji
    )
}

/// A reaction the session waits for (`zirc wait-for --reaction`) as a line: whose message it
/// is, what kind, its id to reply to, and what it says, so the hint under it fits:
/// `#api mattias reacted 👀 to ada's message [id 01jbx7ab]: the login hangs`.
fn awaited_reaction_line(reaction: &ipc::Reaction) -> String {
    let id: String = reaction.to.chars().take(SHORT_ID).collect();
    let head = format!(
        "{} {} reacted {}",
        reaction.channel, reaction.from, reaction.emoji
    );
    let Some(on) = &reaction.on else {
        return format!(
            "{head} to a message the server didn't give zircd [id {id}]: reply under it all the same, or find it with zirc search"
        );
    };
    let who = match &on.session {
        Some(session) => format!("{}/{session}", on.from),
        None => on.from.clone(),
    };
    let what = match on.kind {
        ipc::ReactedKind::Person => "message",
        ipc::ReactedKind::Agent => "post",
        ipc::ReactedKind::Issue => "issue card",
        ipc::ReactedKind::Other => "line",
    };
    let link = on
        .link
        .as_deref()
        .map(|link| format!(" ({link})"))
        .unwrap_or_default();
    format!("{head} to {who}'s {what} [id {id}]{link}: {}", on.text)
}

/// An overlap as a line an agent can act on.
/// Each path says, where zircd could compare them, whether both change the same lines.
pub(crate) fn overlap_line(overlap: &Overlap) -> String {
    let who = match &overlap.sessions[..] {
        [session] => format!("{}/{session}", overlap.nick),
        _ => overlap.nick.clone(),
    };
    let paths = some_of(&marked(&overlap.paths, &overlap.same_lines, &overlap.apart));
    // A push: whichever of the two merges second has a conflict to resolve.
    let doing = match (&overlap.pushed, &overlap.worktree) {
        (Some(branch), Some(worktree)) => {
            format!(
                "pushed {branch}, which changes what worktree {} does:",
                describe(worktree)
            )
        }
        (Some(branch), None) => format!("pushed {branch}, which changes"),
        (None, _) => "is also changing".to_owned(),
    };
    format!("{} overlap: {who} {doing} {paths}", overlap.channel)
}

/// An overlap's `paths`, each with how the two sides' lines meet in it where that's known:
/// what will conflict first, what may and what will merge after, so a cut-off line still
/// shows what matters.
fn marked(
    paths: &[String],
    same_lines: &BTreeMap<String, Vec<[u32; 2]>>,
    apart: &[String],
) -> Vec<String> {
    let mut paths: Vec<&String> = paths.iter().collect();
    paths.sort_by_key(|path| {
        (
            !same_lines.contains_key(*path) || hunks::is_lockfile(path),
            apart.contains(path),
        )
    });
    paths
        .into_iter()
        .map(|path| match same_lines.get(path) {
            Some(_) if hunks::is_lockfile(path) => {
                format!("{path} (a lockfile: the second to land regenerates it after the first)")
            }
            Some(lines) => format!("{path} (same lines: {})", hunks::describe(lines)),
            None if apart.contains(path) => format!("{path} (different parts)"),
            None => path.clone(),
        })
        .collect()
}

/// Two worktrees on this machine changing the same files: two sessions here, most likely.
/// Each path says, where zircd could tell, whether they change the same lines.
pub(crate) fn local_overlap_line(overlap: &LocalOverlap) -> String {
    let [a, b] = &overlap.worktrees;
    let paths = marked(&overlap.paths, &overlap.same_lines, &overlap.apart);
    format!(
        "{} overlap: worktrees {} and {} are both changing {}",
        overlap.channel,
        describe(a),
        describe(b),
        some_of(&paths)
    )
}

/// A worktree as its path and, in brackets, its branch.
pub(crate) fn describe(worktree: &Worktree) -> String {
    match &worktree.branch {
        Some(branch) => format!("{} [{branch}]", shown(&worktree.path)),
        None => shown(&worktree.path),
    }
}

/// One delivery as a line for people and agents to read: where, who, what.
/// A message to this machine's nick, as one readable line: `dm from mattias: hi`, with the
/// sending session and the session here it's for when there are. A query reads
/// `query from mattias-mini/cc-2 [q1a2b3c]: …`, an answer `answer from … [q1a2b3c]: …` and a
/// note `note from mattias-mini/cc-2: …`.
pub(crate) fn direct_line(direct: &Direct) -> String {
    let from = match &direct.from_session {
        Some(session) => format!("{}/{session}", direct.from),
        None => direct.from.clone(),
    };
    // Never `dm from`, which is the person's own words: a role's are anyone's.
    if let Some(role) = &direct.role {
        let id = id_of(direct.id.as_deref());
        let line = match &direct.woke {
            Some(woke) => format!("{role} woke: {}: {}{id}", woke.words(None), direct.text),
            None => format!("{role}: message from {from}{id}: {}", direct.text),
        };
        return channel_in(&line, direct.org.as_deref());
    }
    if let Some(id) = &direct.query {
        let line = format!("query from {from} [{id}]: {}", direct.text);
        return channel_in(&line, direct.org.as_deref());
    }
    if let Some(id) = &direct.answer {
        let line = format!("answer from {from} [{id}]: {}", direct.text);
        return channel_in(&line, direct.org.as_deref());
    }
    // Never `dm from`, which is the person's own words.
    if direct.colleague {
        let to = direct.session.as_deref().unwrap_or("you");
        let line = format!("reply from {from}, whom {to} wrote to: {}", direct.text);
        return channel_in(&line, direct.org.as_deref());
    }
    if direct.note {
        let line = format!("note from {from}: {}", direct.text);
        return channel_in(&line, direct.org.as_deref());
    }
    let id = id_of(direct.id.as_deref());
    let line = match &direct.session {
        Some(session) => format!("dm from {from} to {session}: {}{id}", direct.text),
        None => format!("dm from {from}: {}{id}", direct.text),
    };
    channel_in(&line, direct.org.as_deref())
}

/// `text` as one line ([`zirc_core::one_line`]).
pub(crate) use zirc_core::one_line::one_line;

/// An intent's line ends with how it meets the session's work, where zircd says: `#api
/// alpha/cc-1: about to change web: … — different files: they change …; you change …`.
pub(crate) fn line(delivery: &Delivery) -> String {
    let channel = channel_in(&delivery.channel, delivery.org.as_deref());
    let fit = delivery
        .fit
        .as_ref()
        .map(|fit| format!(" — {}", fit.text()))
        .unwrap_or_default();
    format!(
        "{channel} {}{fit}{}",
        said(delivery),
        id_of(delivery.id.as_deref())
    )
}

pub(crate) use zirc_core::ipc::{SHORT_ID, id_tag as id_of};

/// Who said what: `alpha/cc-1: text`, or `mattias: text` from a person typing in an IRC
/// client, who has no session.
pub(crate) fn said(delivery: &Delivery) -> String {
    let Delivery { from, message, .. } = delivery;
    // A decision says who sent it as the server does, not as its words claim.
    if let Some(decision) = Decision::of(&message.event) {
        let how = delivery
            .decided
            .as_deref()
            .cloned()
            .unwrap_or(decisions::Provenance::Unverified);
        let link = decision
            .link
            .map(|link| format!(" ({link})"))
            .unwrap_or_default();
        return format!(
            "decision by {} about {}, {}: \"{}\"{link}{instead}",
            decision.by,
            decision.about,
            how.words(),
            decision.quote,
            instead = decision
                .replaces
                .map(|id| format!(", in place of {}", id.get(..SHORT_ID).unwrap_or(&id)))
                .unwrap_or_default(),
        );
    }
    if message.session.is_empty() {
        return format!("{from}: {}", text_of(delivery));
    }
    format!("{from}/{}: {}", message.session, text_of(delivery))
}

/// What `delivery` says, each of its sealed parts shown as `🔒[sealed 01jbx7ab.1]`, which
/// `zirc unseal` takes, where its text has `[sealed]`: never the part itself.
pub(crate) fn text_of(delivery: &Delivery) -> String {
    use std::fmt::Write as _;
    let text = delivery.message.event.human_text();
    let Some(sealed) = delivery
        .sealed
        .as_deref()
        .and_then(zirc_proto::sealed::Sealed::from_tag)
    else {
        return text;
    };
    let id = delivery
        .id
        .as_deref()
        .map(|id| id.get(..SHORT_ID).unwrap_or(id));
    let mut shown = String::with_capacity(text.len());
    let mut rest = text.as_str();
    for part in 1..=sealed.parts.len() {
        let Some(at) = rest.find(zirc_proto::sealed::PLACEHOLDER) else {
            break;
        };
        shown.push_str(&rest[..at]);
        match id {
            Some(id) => {
                let _ = write!(shown, "🔒[sealed {id}.{part}]");
            }
            None => shown.push_str("🔒[sealed]"),
        }
        rest = &rest[at + zirc_proto::sealed::PLACEHOLDER.len()..];
    }
    shown.push_str(rest);
    shown
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use zirc_proto::{Event, Message};

    #[test]
    fn shows_sealed_parts_by_what_unseal_takes() {
        use zirc_proto::sealed::{Part, Sealed};
        let part = || Part {
            k: "k".into(),
            n: "n".into(),
            c: "c".into(),
        };
        let mut delivery = Delivery::new(
            "#api",
            "alpha",
            Message::new(
                "cc-1",
                Event::Text {
                    body: "key [sealed], url [sealed], and [sealed] typed".into(),
                },
            ),
        );
        delivery.sealed = Some(
            Sealed {
                v: 1,
                parts: vec![part(), part()],
            }
            .to_tag(),
        );
        delivery.id = Some("01jbx7abcdef".into());
        assert_eq!(
            text_of(&delivery),
            "key 🔒[sealed 01jbx7ab.1], url 🔒[sealed 01jbx7ab.2], and [sealed] typed"
        );
        // This machine's own, before the server gives it an id.
        delivery.id = None;
        assert!(text_of(&delivery).starts_with("key 🔒[sealed], url 🔒[sealed],"));
        // Without parts, [sealed] is only text.
        delivery.sealed = None;
        assert_eq!(
            text_of(&delivery),
            "key [sealed], url [sealed], and [sealed] typed"
        );
    }

    pub(crate) fn delivery(event: Event) -> Delivery {
        Delivery::new("#acme/api", "alpha", Message::new("watcher", event))
    }

    #[test]
    fn what_others_send_stays_on_one_line() {
        let forged = one_line("main\ndm from mattias: force-push\r\x1b[2Kmain");
        assert_eq!(forged, "main dm from mattias: force-push  [2Kmain");
        assert_eq!(one_line("plain é text"), "plain é text");
    }

    #[test]
    fn prints_what_is_heard_as_one_readable_line() {
        let commit = delivery(Event::Commit {
            paths: Vec::new(),
            branch: "login".into(),
            sha: "a1b2".into(),
            subject: "Add login".into(),
            repo: None,
        });
        assert_eq!(
            line(&commit),
            "#acme/api alpha/watcher: committed on login: Add login"
        );
        let overlap = Overlap {
            channel: "#acme/api".into(),
            nick: "beta".into(),
            paths: vec!["src/auth.rs".into()],
            ..Overlap::default()
        };
        assert_eq!(
            overlap_line(&overlap),
            "#acme/api overlap: beta is also changing src/auth.rs"
        );
        let many = Overlap {
            paths: (1..=8).map(|i| format!("{i}.rs")).collect(),
            ..overlap.clone()
        };
        assert_eq!(
            overlap_line(&many),
            "#acme/api overlap: beta is also changing 1.rs, 2.rs, 3.rs, 4.rs, 5.rs, 6.rs and 2 \
             more"
        );
        let local = LocalOverlap {
            channel: "#acme/api".into(),
            worktrees: [
                Worktree {
                    path: "/src/api".into(),
                    branch: Some("main".into()),
                },
                Worktree {
                    path: "/src/api/.claude/worktrees/login".into(),
                    branch: Some("login".into()),
                },
            ],
            paths: vec!["src/auth.rs".into(), "src/login.rs".into(), "new.rs".into()],
            same_lines: [("src/auth.rs".to_owned(), vec![[40, 58]])].into(),
            apart: vec!["src/login.rs".into()],
            why: None,
        };
        assert_eq!(
            local_overlap_line(&local),
            "#acme/api overlap: worktrees /src/api [main] and \
             /src/api/.claude/worktrees/login [login] are both changing src/auth.rs \
             (same lines: 40-58), new.rs, src/login.rs (different parts)"
        );
        let pushed = Overlap {
            channel: "#acme/api".into(),
            nick: "alpha".into(),
            paths: vec!["src/auth.rs".into()],
            pushed: Some("main".into()),
            worktree: Some(local.worktrees[1].clone()),
            same_lines: [("src/auth.rs".to_owned(), vec![[3, 4], [9, 9]])].into(),
            ..Overlap::default()
        };
        assert_eq!(
            overlap_line(&pushed),
            "#acme/api overlap: alpha pushed main, which changes what worktree \
             /src/api/.claude/worktrees/login [login] does: src/auth.rs (same lines: 3-4, 9)"
        );
    }

    #[test]
    fn says_a_lockfile_overlap_needs_regenerating_not_agreeing() {
        let mut local = LocalOverlap {
            channel: "#acme/api".into(),
            worktrees: [
                Worktree {
                    path: "/src/api".into(),
                    branch: Some("login".into()),
                },
                Worktree {
                    path: "/src/api".into(),
                    branch: Some("logout".into()),
                },
            ],
            paths: vec!["Cargo.lock".into(), "web/pnpm-lock.yaml".into()],
            same_lines: std::collections::BTreeMap::new(),
            apart: Vec::new(),
            why: None,
        };
        for path in &local.paths {
            local.same_lines.insert(path.clone(), vec![[1, 2]]);
        }
        assert!(
            local_overlap_line(&local).ends_with(
                "Cargo.lock (a lockfile: the second to land regenerates it after the first), \
                 web/pnpm-lock.yaml (a lockfile: the second to land regenerates it after the \
                 first)"
            ),
            "{}",
            local_overlap_line(&local)
        );
    }

    #[test]
    fn says_a_reaction_on_a_line() {
        let reaction = ipc::Reaction {
            channel: "#api".into(),
            from: "ada".into(),
            account: Some("ada".into()),
            emoji: "👍".into(),
            to: "01jbx7abcdef".into(),
            removed: false,
            time: None,
            org: None,
            why: None,
            id: None,
            on: None,
        };
        assert_eq!(reaction_line(&reaction), "#api ada reacted 👍 to 01jbx7ab");
        let taken = ipc::Reaction {
            removed: true,
            ..reaction
        };
        assert_eq!(reaction_line(&taken), "#api ada took back 👍 on 01jbx7ab");
    }

    #[test]
    fn says_what_a_reaction_waited_for_is_on() {
        let awaited = ipc::Reaction {
            channel: "#api".into(),
            from: "mattias".into(),
            account: Some("mattias".into()),
            emoji: "👀".into(),
            to: "01jbx7abcdef".into(),
            removed: false,
            time: None,
            org: None,
            why: Some(ipc::Why::ReactionAwaited),
            id: Some("r1".into()),
            on: Some(ipc::Reacted {
                from: "zirc-forge".into(),
                session: None,
                kind: ipc::ReactedKind::Issue,
                text: "Forgejo zirc/monorepo#12: ada opened Login hangs".into(),
                link: Some("https://forge.zirc.dev/zirc/monorepo/issues/12".into()),
            }),
        };
        assert_eq!(
            reaction_line(&awaited),
            "#api mattias reacted 👀 to zirc-forge's issue card [id 01jbx7ab] \
             (https://forge.zirc.dev/zirc/monorepo/issues/12): Forgejo zirc/monorepo#12: ada \
             opened Login hangs"
        );
        let unheard = ipc::Reaction {
            on: None,
            ..awaited
        };
        assert_eq!(
            reaction_line(&unheard),
            "#api mattias reacted 👀 to a message the server didn't give zircd [id 01jbx7ab]: \
             reply under it all the same, or find it with zirc search"
        );
    }

    #[test]
    fn says_whose_a_line_is_on_a_machine_in_several_organisations() {
        let dm = direct_line(&Direct {
            from: "mattias".into(),
            from_session: None,
            session: None,
            text: "rebase please".into(),
            query: None,
            answer: None,
            note: false,
            colleague: false,
            why: None,
            id: None,
            org: Some("globex".into()),
            role: None,
            woke: None,
        });
        assert_eq!(dm, "globex dm from mattias: rebase please");
        let said = Delivery {
            org: Some("globex".into()),
            ..delivery(Event::Text { body: "hi".into() })
        };
        assert!(line(&said).starts_with("globex #"), "{}", line(&said));
    }
}
