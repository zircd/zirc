//! Forgetting what this machine kept of organisations it's no longer in: their leases, which
//! name other members' machines and sessions, and their team's hints. Done as zircd starts,
//! as leaving one, by `zirc logout <org>` or by `zirc login --force` to another, restarts it.
//!
//! The first organisation keeps its things in zirc's home itself ([`zirc_core::paths::org`]),
//! so which one that was is kept beside them ([`FIRST`]): once another is first, the leases
//! there were the other's. The repositories it watches and the topics it joined stay, as the
//! person's own.

use std::path::Path;

use zirc_core::paths;

/// The file in zirc's home that names the organisation whose leases are kept there.
pub(crate) const FIRST: &str = "first-org";

/// Forgets what's kept in `home` of organisations not in `orgs`, the first first. A machine
/// that kept no name yet, as before this was, keeps its home's leases as the first's.
pub(crate) fn forget(home: &Path, orgs: &[String]) {
    let Some(first) = orgs.first() else {
        return;
    };
    let marker = home.join(FIRST);
    let was = std::fs::read_to_string(&marker).ok();
    if was.as_deref().is_some_and(|was| was.trim() != first) {
        let _ = std::fs::remove_file(paths::leases(home));
    }
    if was.as_deref().map(str::trim) != Some(first.as_str()) {
        let _ = std::fs::write(&marker, first);
    }
    let kept = |name: &str| orgs.iter().any(|org| org == name);
    for entry in std::fs::read_dir(home.join("orgs"))
        .into_iter()
        .flatten()
        .flatten()
    {
        if !kept(&entry.file_name().to_string_lossy()) {
            let _ = std::fs::remove_file(paths::leases(&entry.path()));
        }
    }
    for entry in std::fs::read_dir(home.join("team-hints"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.strip_suffix(".json").is_some_and(|org| !kept(org)) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn orgs(names: &[&str]) -> Vec<String> {
        names.iter().map(|&name| name.to_owned()).collect()
    }

    /// A home whose first organisation was `first`, with leases for it, `globex` and `initech`,
    /// and hints of each.
    fn home(first: Option<&str>) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        if let Some(first) = first {
            std::fs::write(home.path().join(FIRST), first).unwrap();
        }
        std::fs::write(paths::leases(home.path()), "{}").unwrap();
        std::fs::create_dir_all(home.path().join("team-hints")).unwrap();
        for org in ["globex", "initech"] {
            std::fs::create_dir_all(paths::org(home.path(), org)).unwrap();
            std::fs::write(paths::leases(&paths::org(home.path(), org)), "{}").unwrap();
        }
        for org in ["zirc", "globex", "initech"] {
            std::fs::write(zirc_core::team_hints::path(home.path(), org), "{}").unwrap();
        }
        home
    }

    #[test]
    fn forgets_the_leases_and_hints_of_organisations_left() {
        let home = home(Some("zirc"));

        forget(home.path(), &orgs(&["zirc", "globex"]));

        assert!(paths::leases(home.path()).exists());
        assert!(paths::leases(&paths::org(home.path(), "globex")).exists());
        assert!(!paths::leases(&paths::org(home.path(), "initech")).exists());
        assert!(zirc_core::team_hints::path(home.path(), "globex").exists());
        assert!(!zirc_core::team_hints::path(home.path(), "initech").exists());
    }

    #[test]
    fn another_first_organisation_forgets_the_leases_kept_in_home() {
        // Logged in to globex alone, with --force, after zirc.
        let home = home(Some("zirc"));

        forget(home.path(), &orgs(&["globex"]));

        assert!(!paths::leases(home.path()).exists());
        assert!(!zirc_core::team_hints::path(home.path(), "zirc").exists());
        assert_eq!(
            std::fs::read_to_string(home.path().join(FIRST)).unwrap(),
            "globex"
        );
    }

    #[test]
    fn a_home_that_named_no_organisation_keeps_its_leases_as_the_firsts() {
        let home = home(None);

        forget(home.path(), &orgs(&["zirc"]));

        assert!(paths::leases(home.path()).exists());
        assert_eq!(
            std::fs::read_to_string(home.path().join(FIRST)).unwrap(),
            "zirc"
        );
    }
}
