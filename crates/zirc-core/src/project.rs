//! A repository as zirc names it. Every repository's work goes to the organisation's one
//! [`AGENTS_CHANNEL`](zirc_proto::AGENTS_CHANNEL), each line saying its repository by
//! [`Project::repo`]: clones of one project share a remote, so the remote's repository name
//! names it the same on every machine. Two repositories with the same name are one to zirc.
//! Topics people join are the organisation's, `#auth` ([`topic_channel`]). Before 20 Nov 2026
//! each repository had channels of its own (`#api`, `#api/agents`, `#api/<branch>`…): they're
//! left as they were, and nothing makes or reads them.

use zirc_chat::names::channel;

/// The organisation-wide channel, which zirc-chat puts new people in.
pub use zirc_chat::chat::ORG_CHANNEL;

/// A repository, as zirc names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// The repository's name, in lowercase with only what channel names allow, as each
    /// line about it says it ([`zirc_proto::Message::repo`]).
    pub repo: String,
    /// Its web address on a forge, such as `https://github.com/acme/api`, from its remote
    /// ([`web_address`]), so apps can link its commits, branches and files.
    pub web: Option<String>,
}

impl Project {
    /// The project for a repository with remote `url`, or, without a usable remote, for a
    /// repository in a directory called `dir_name`.
    #[must_use]
    pub fn new(remote_url: Option<&str>, dir_name: &str) -> Self {
        let name = remote_url.and_then(repo_name);
        Self {
            repo: clean(name.as_deref().unwrap_or(dir_name)),
            web: remote_url.and_then(web_address),
        }
    }

    /// `#api`: how hints and shared states have named the repository since it had a
    /// channel of that name, and how zircd keys what's said about it. It's no channel now.
    #[must_use]
    pub fn scope(&self) -> String {
        scope(&self.repo)
    }
}

/// The scope ([`Project::scope`]) of the repository named `repo`, as a line about it says
/// it ([`zirc_proto::Message::repo`]): `#api` for `api`.
pub use zirc_chat::names::repo_scope as scope;

/// The organisation's channel for the topic `topic`, `#auth`.
#[must_use]
pub fn topic_channel(topic: &str) -> String {
    channel(&clean(topic))
}

/// Lowercase, with only what channel names and URLs agree on, as every channel part is
/// named ([`zirc_chat::names::channel_part`]).
fn clean(name: &str) -> String {
    zirc_chat::names::channel_part(name)
}

/// The repository's name in the forms Git remotes come in: `git@host:owner/repo.git`,
/// `https://host/owner/repo`, `ssh://git@host:22/owner/repo.git` and local paths alike.
/// The forges whose repositories have a web page at `https://<host>/<owner>/<repo>`.
const FORGES: [&str; 5] = [
    "github.com",
    "gitlab.com",
    "bitbucket.org",
    "codeberg.org",
    "git.sr.ht",
];

/// The web address of the repository with remote `url`, such as `https://github.com/acme/api`
/// for `git@github.com:acme/api.git`: on a forge it knows ([`FORGES`]), over https, without a
/// user, password, token, port or `.git`. `None` for anything else, such as a path or a
/// server it can't tell has web pages. `ops/say.sh` (the monorepo's) makes the same address
/// for CI's events, in shell; change both.
#[must_use]
pub fn web_address(url: &str) -> Option<String> {
    // A team's own forge, such as `git.example.com`, is named by the environment, never here:
    // this crate can't know anyone's. `ZIRC_FORGE_HOSTS` is a comma-separated list.
    let extra = std::env::var("ZIRC_FORGE_HOSTS").unwrap_or_default();
    web_address_on(
        url,
        extra.split(',').map(str::trim).filter(|h| !h.is_empty()),
    )
}

/// [`web_address`] for the forges it knows and the hosts of `extra`, such as a team's own.
#[must_use]
pub fn web_address_on<'a>(url: &str, extra: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let url = url.trim();
    let (host, path) = match url.split_once("://") {
        Some((scheme, rest)) => {
            if !matches!(scheme, "https" | "http" | "ssh" | "git" | "git+ssh") {
                return None;
            }
            rest.split_once('/')?
        }
        // scp-like: [user@]host:path, as long as it's not a Windows path.
        None => url.split_once(':').filter(|(host, _)| host.len() > 1)?,
    };
    // Credentials and a port aren't part of the address.
    let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
    let host = host
        .split_once(':')
        .map_or(host, |(host, _)| host)
        .to_lowercase();
    if !FORGES.contains(&host.as_str()) && !extra.into_iter().any(|h| h.eq_ignore_ascii_case(&host))
    {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    let parts: Vec<&str> = path.split('/').collect();
    let plain = |part: &&str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~'))
    };
    (parts.len() >= 2 && parts.iter().all(plain)).then(|| format!("https://{host}/{path}"))
}

fn repo_name(url: &str) -> Option<String> {
    let path = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/').map_or("", |(_, path)| path),
        None => url.rsplit_once(':').map_or(url, |(_, path)| path),
    };
    path.trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit('/')
        .find(|part| !part.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zirc_chat::names::MAX_CHANNEL_BYTES;

    fn api() -> Project {
        Project::new(Some("git@github.com:Acme/API.git"), "checkout")
    }

    #[test]
    fn names_projects_after_the_remote() {
        for url in [
            "git@github.com:Acme/API.git",
            "https://github.com/acme/api",
            "https://github.com/acme/api.git/",
            "ssh://git@github.com:22/acme/api.git",
            "/srv/git/acme/api.git",
        ] {
            assert_eq!(
                Project::new(Some(url), "checkout").repo,
                api().repo,
                "{url}"
            );
        }
    }

    #[test]
    fn knows_a_repositorys_web_address_without_its_credentials() {
        for url in [
            "git@github.com:Acme/API.git",
            "ssh://git@github.com:22/Acme/API.git",
            "https://github.com/Acme/API",
            "https://github.com/Acme/API.git/",
            "http://github.com/Acme/API.git",
            "https://x-access-token:ghs_secret@github.com/Acme/API.git",
            "https://mattias:hunter2@GitHub.com/Acme/API",
        ] {
            let web = web_address(url);
            assert_eq!(web.as_deref(), Some("https://github.com/Acme/API"), "{url}");
            assert!(!web.unwrap().contains("secret"));
        }
        assert_eq!(
            web_address("git@gitlab.com:group/sub/project.git").as_deref(),
            Some("https://gitlab.com/group/sub/project")
        );
        for url in [
            "/srv/git/acme/api.git",
            "file:///srv/git/api.git",
            "C:/repos/api",
            "git@git.internal.example:acme/api.git",
            "https://github.com/acme",
            "https://github.com/acme/api?token=secret",
            "",
        ] {
            assert_eq!(web_address(url), None, "{url}");
        }
        assert_eq!(api().web.as_deref(), Some("https://github.com/Acme/API"));
    }

    #[test]
    fn names_the_repository_and_the_organisations_topics() {
        let project = api();
        assert_eq!(project.repo, "api");
        assert_eq!(project.scope(), "#api");
        assert_eq!(topic_channel("Auth"), "#auth");
    }

    #[test]
    fn falls_back_to_the_directory_name() {
        assert_eq!(Project::new(None, "My Project").repo, "my-project");
        assert_eq!(Project::new(Some(""), "api").scope(), "#api");
    }

    #[test]
    fn keeps_names_within_a_channels_limits() {
        let long = "x".repeat(100);
        assert_eq!(Project::new(None, &long).scope().len(), MAX_CHANNEL_BYTES);
        assert!(topic_channel(&"é".repeat(40)).len() <= MAX_CHANNEL_BYTES);
    }

    #[test]
    fn a_teams_own_forge_has_web_pages_only_when_named() {
        let url = "ssh://git@git.example.com:2222/acme/api.git";
        assert_eq!(web_address_on(url, []), None);
        assert_eq!(
            web_address_on(url, ["git.example.com"]).as_deref(),
            Some("https://git.example.com/acme/api")
        );
        assert_eq!(
            web_address_on("https://GIT.example.com/acme/api", ["git.example.com"]).as_deref(),
            Some("https://git.example.com/acme/api")
        );
        // A host that only contains the name, or a path that does, is not it.
        for url in [
            "https://git.example.com.evil.test/acme/api",
            "https://evil.test/git.example.com/acme/api",
        ] {
            assert_eq!(web_address_on(url, ["git.example.com"]), None, "{url}");
        }
        // The known forges stay known.
        assert_eq!(
            web_address_on("git@github.com:acme/api.git", ["git.example.com"]).as_deref(),
            Some("https://github.com/acme/api")
        );
    }
}
