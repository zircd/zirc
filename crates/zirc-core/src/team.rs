//! The team's own configuration for a repository, checked into it at
//! `.zirc/config.toml`, so every machine and session working on it agrees, and a change to
//! it goes through review as any change does. Everything in it has a default; a repository
//! without the file has those. It says where an agent's plain `zirc post` goes:
//!
//! ```toml
//! [post]
//! default = "agents"   # #<repo>/agents, the default; "repo" for the repository's channel
//!
//! [rooms.areas]        # none by default
//! web = ["web/", "oss/web/"]
//! apps = ["oss/apps/"]
//! ```
//!
//! Only a post for a person goes to the repository's channel, which is its people's
//! ([`for_people`]): one naming or addressing a person. A status line ([`STATUS`]) and
//! everything agents say to each other, `@everyone` included, goes to the agents' room,
//! unless the team has areas: then a post from a worktree whose changes are mostly in one
//! area's paths goes to that area's room, the repository's topic of its name
//! ([`Team::area_of`]).

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

/// Where the team's configuration is, from the repository's root.
pub const PATH: &str = ".zirc/config.toml";

/// The channel agents post to unless the team says otherwise: `#<repo>/agents`, zirc-chat's
/// [`AGENTS_ROOM`](crate::chat::AGENTS_ROOM), which every app counts as the repository's.
pub const AGENTS: &str = crate::chat::AGENTS_ROOM;

/// What [`Post::default`] says for the repository's own channel.
pub const REPO: &str = "repo";

/// Why a repository's team configuration couldn't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file is there, but couldn't be read.
    #[error("couldn't read {0}: {1}")]
    Read(String, std::io::Error),
    /// It isn't what [`Team`] describes.
    #[error("{0}: {1}")]
    Parse(String, toml::de::Error),
}

/// A repository's team configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Team {
    /// Where posts go.
    #[serde(default)]
    pub post: Post,
    /// The repository's rooms.
    #[serde(default)]
    pub rooms: Rooms,
}

/// The repository's rooms.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rooms {
    /// Its areas, by name, each with the paths it covers: files, or directories such as
    /// `web/`, relative to the repository's root. Each is a topic of the repository's,
    /// `#<repo>/<area>`, where posts about its paths go.
    #[serde(default)]
    pub areas: BTreeMap<String, Vec<String>>,
}

/// Where posts go.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Post {
    /// Where a plain `zirc post` goes: a topic of the repository's, [`AGENTS`] unless set,
    /// or [`REPO`] for the repository's channel.
    pub default: Option<String>,
}

/// Where a plain post goes, as [`Team::room`] says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Room {
    /// The repository's channel.
    Repo,
    /// The repository's topic of this name.
    Topic(String),
}

impl Team {
    /// The team configuration of the repository at `root`: the defaults without a file.
    ///
    /// # Errors
    ///
    /// When the file is there but can't be read, or isn't what this module describes.
    pub fn load(root: &Path) -> Result<Self, Error> {
        let path = root.join(PATH);
        let shown = || path.display().to_string();
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|err| Error::Parse(shown(), err)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(Error::Read(shown(), err)),
        }
    }

    /// Where a plain post goes.
    #[must_use]
    pub fn room(&self) -> Room {
        match self.post.default.as_deref().map(str::trim) {
            Some(REPO) => Room::Repo,
            Some(topic) if !topic.is_empty() => Room::Topic(topic.to_owned()),
            _ => Room::Topic(AGENTS.to_owned()),
        }
    }

    /// The names of its areas, which are topics of the repository's.
    pub fn areas(&self) -> impl Iterator<Item = &str> {
        self.rooms
            .areas
            .iter()
            .filter(|(name, prefixes)| !name.trim().is_empty() && !prefixes.is_empty())
            .map(|(name, _)| name.as_str())
    }

    /// The area most of `paths` are in, each counted for the area whose path covers it most
    /// closely: `None` without areas, when none is in one, or when two areas have as many.
    #[must_use]
    pub fn area_of<'a, I>(&self, paths: I) -> Option<&str>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for path in paths {
            let closest = self
                .rooms
                .areas
                .iter()
                .filter(|(area, _)| !area.trim().is_empty())
                .filter_map(|(area, prefixes)| {
                    prefixes
                        .iter()
                        .map(|prefix| prefix.trim_end_matches('/'))
                        .filter(|prefix| covers(prefix, path))
                        .map(str::len)
                        .max()
                        .map(|len| (len, area.as_str()))
                })
                .max_by_key(|(len, _)| *len);
            if let Some((_, area)) = closest {
                *counts.entry(area).or_default() += 1;
            }
        }
        let most = counts.values().copied().max()?;
        let mut top = counts.into_iter().filter(|(_, count)| *count == most);
        match (top.next(), top.next()) {
            (Some((area, _)), None) => Some(area),
            _ => None,
        }
    }
}

/// Whether `prefix`, a file or directory without its trailing `/`, covers `path`.
fn covers(prefix: &str, path: &str) -> bool {
    !prefix.is_empty()
        && path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Whether `session` may post `text` to the repository's channel with `--to repo`: anyone
/// but an agent session may, and a session only for a person ([`for_people`]). The channel
/// is its people's: a session's findings, diagnoses and questions to no one belong in the
/// agents' room, or in a note or a `zirc dm` to whom they concern. On 5 Nov 2026, 96 of the
/// repository channel's 100 lines were sessions'.
#[must_use]
pub fn may_post_to_repo(session: &str, text: &str, people: &[&str], agents: &[&str]) -> bool {
    !agents.iter().any(|it| it.eq_ignore_ascii_case(session)) || for_people(text, people, agents)
}

/// The labels that start a session's status line, which is for the agents' room.
pub const STATUS: [&str; 3] = ["taking:", "done:", "dropped:"];

/// Whether `text` is a session's status line: one starting [`STATUS`].
#[must_use]
pub fn is_status(text: &str) -> bool {
    let text = text.trim_start();
    STATUS.iter().any(|label| {
        text.get(..label.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(label))
    })
}

/// Whether a post saying `text` is for a person, and so for the repository's channel, which
/// is its people's, whatever the team's default: it's addressed to someone not among
/// `agents` (`ada: …`), names one of `people` as a word of its own, or `@`s someone not
/// among `agents`. `@everyone` is every agent, so it isn't; nor is a question to nobody, which
/// is how agents ask each other: one for a person names them, or is a `zirc request`. A
/// status line, one starting [`STATUS`] (`taking:`, `done:`, `dropped:`), never is, whoever
/// it names: it's how sessions tell each other what they've taken on.
#[must_use]
pub fn for_people(text: &str, people: &[&str], agents: &[&str]) -> bool {
    let text = text.trim();
    // A session's `done: … for Mattias` once landed in #monorepo (6 Nov 2026).
    if is_status(text) {
        return false;
    }
    let agent = |name: &str| agents.iter().any(|it| it.eq_ignore_ascii_case(name));
    let person = |name: &str| people.iter().any(|it| it.eq_ignore_ascii_case(name));
    // Addressed to someone, `mattias: …`, only when it's a person known here: a label such
    // as `heads-up:`, `fyi:` or `note:` is agents talking (5 Nov 2026, a heads-up landed in
    // #monorepo).
    if let Some((to, _)) = text.split_once(": ")
        && person(to.trim_start_matches('@'))
    {
        return true;
    }
    text.split_whitespace().any(|word| {
        let Some(name) = word.strip_prefix('@') else {
            return false;
        };
        let name = name.trim_end_matches(|c: char| !c.is_alphanumeric());
        !name.is_empty() && !name.eq_ignore_ascii_case("everyone") && !agent(name)
    }) || people
        .iter()
        .any(|name| !name.is_empty() && crate::names::mentions(text, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_posts_go_to_the_agents_channel_unless_the_team_says() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Team::load(dir.path()).unwrap().room(),
            Room::Topic("agents".into())
        );
        std::fs::create_dir(dir.path().join(".zirc")).unwrap();
        let config = dir.path().join(PATH);
        std::fs::write(&config, "[post]\ndefault = \"repo\"\n").unwrap();
        assert_eq!(Team::load(dir.path()).unwrap().room(), Room::Repo);
        std::fs::write(&config, "[post]\ndefault = \"bots\"\n").unwrap();
        assert_eq!(
            Team::load(dir.path()).unwrap().room(),
            Room::Topic("bots".into())
        );
        std::fs::write(&config, "[post]\nchannel = \"x\"\n").unwrap();
        assert!(
            Team::load(dir.path()).is_err(),
            "an unknown key is a mistake to say"
        );
    }

    #[test]
    fn a_post_goes_to_the_area_most_of_the_work_is_in() {
        let team: Team = toml::from_str(
            "[rooms.areas]\n\
             web = [\"web/\", \"oss/web/\"]\n\
             apps = [\"oss/apps/\", \"oss/crates/zirc-tui\"]\n\
             platform = [\"oss/crates/\", \"ops/\"]\n",
        )
        .unwrap();
        assert_eq!(
            team.areas().collect::<Vec<_>>(),
            ["apps", "platform", "web"]
        );
        let area = |paths: &[&str]| team.area_of(paths.iter().copied());
        assert_eq!(area(&["web/src/app.ts", "oss/web/x.ts"]), Some("web"));
        assert_eq!(area(&["web/a", "web/b", "ops/land.sh"]), Some("web"));
        // The closest path wins: zirc-tui is the apps', though it's under crates/.
        assert_eq!(
            area(&[
                "oss/crates/zirc-tui/src/lib.rs",
                "oss/crates/zircd/src/x.rs",
                "oss/apps/a"
            ]),
            Some("apps")
        );
        assert_eq!(area(&["oss/crates/zirc-tui"]), Some("apps"));
        // A directory covers what's in it, not what only starts the same.
        assert_eq!(area(&["webby/x", "ops-old/y"]), None);
        assert_eq!(area(&["web/a", "ops/b"]), None, "a tie is no area's");
        assert_eq!(area(&["README.md"]), None);
        assert_eq!(area(&[]), None);
        assert_eq!(Team::default().area_of(["web/a"]), None);
        assert!(
            toml::from_str::<Team>("[rooms]\nchannels = []\n").is_err(),
            "an unknown key is a mistake to say"
        );
    }

    #[test]
    fn a_status_line_starts_with_its_label() {
        assert!(is_status("taking: the login page"));
        assert!(is_status("  Done: it"));
        assert!(is_status("dropped: it"));
        assert!(!is_status("cc-1a2b3c: done: it"));
        assert!(!is_status("undone: it"));
    }

    #[test]
    fn a_session_posts_to_the_repository_channel_only_for_a_person() {
        let people = ["mattias"];
        let agents = ["cc-1a2b3c"];
        let may = |session: &str, text: &str| may_post_to_repo(session, text, &people, &agents);
        assert!(may("cc-1a2b3c", "mattias: shall I publish 0.1.26?"));
        assert!(may("cc-1a2b3c", "@mattias the release is ready"));
        assert!(!may(
            "cc-1a2b3c",
            "build 2262 fails on tests my change doesn't touch"
        ));
        assert!(!may("cc-1a2b3c", "taking: the login page"));
        // A person at a terminal says what they like, and so does one we know nothing of.
        assert!(may("mattias", "build 2262 is red"));
        assert!(may("someone", "anything"));
    }

    #[test]
    fn only_a_post_for_a_person_goes_to_the_repository_channel() {
        let people = ["mattias", "ada"];
        let agents = ["cc-1a2b3c", "desktop", "mattias-mbp"];
        for text in [
            "mattias: shall I publish 0.1.26?",
            "ada, the build is green for you",
            "@ada have a look",
            "@mattias the release is ready",
            "heads-up: mattias, I unapplied your branch",
        ] {
            assert!(for_people(text, &people, &agents), "{text}");
        }
        for text in [
            "taking: the login page",
            "done: the login page",
            "done: drafted the headline as a Claude Doc for Mattias",
            "Taking: @ada's review comments",
            "dropped: mattias's rename, it's in ada's branch",
            "desktop: can you rebase onto it?",
            "cc-1a2b3c: landed",
            "mattias-mbp: your zircd restarted",
            "@everyone: master is frozen",
            "@desktop take a look",
            "is anyone in daemon.rs?",
            "building the release, 2 of 4",
            "an email@address isn't anyone",
            "heads-up: I unapplied web-review5 to free the workspace",
            "fyi: the queue is moving again",
            "note: master moved",
            "bob: are you still on the login page?",
        ] {
            assert!(!for_people(text, &people, &agents), "{text}");
        }
    }
}
