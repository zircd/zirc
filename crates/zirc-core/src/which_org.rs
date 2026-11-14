//! Which organisation a command acts on, one rule for zircd's router and the CLI's own reads
//! from zirc.dev alike. `zirc status`, `zirc orgs` and `zirc watch` cover every organisation;
//! every other command acts on one: the one `--org` names, else the one the repository it's
//! run in belongs to, else the machine's only own organisation. A guest's terminal and
//! zirc.dev's public server are never taken for one the command didn't name, unless the
//! machine has no own organisation. Otherwise it fails, asking for `--org`.

use std::fmt;

/// An organisation a command may act on, as the picker sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice<'a> {
    /// Its name, as `--org` takes it.
    pub name: &'a str,
    /// Whether it's the machine's own, rather than a guest's terminal or the public server:
    /// only those are taken without being named, unless there are none.
    pub own: bool,
}

/// Why no organisation was picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unpicked {
    /// `--org` named one the machine isn't signed in to.
    NotIn {
        /// The one named.
        name: String,
        /// The ones it's signed in to.
        orgs: Vec<String>,
    },
    /// Nothing says which of several it's for.
    Several(Vec<String>),
    /// The machine isn't signed in to any.
    None,
}

/// What [`Unpicked::Several`] tells someone to add: `zirc post`, whose `--org` was taken
/// for its topics, says `--in` instead.
pub const SAY_WHICH: &str = "say which with --org";

impl fmt::Display for Unpicked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotIn { name, orgs } => write!(
                f,
                "this machine isn't signed in to {name}, only to {}",
                and(orgs)
            ),
            Self::Several(orgs) => write!(f, "this machine is in {}: {SAY_WHICH}", and(orgs)),
            Self::None => write!(
                f,
                "this machine isn't signed in to an organisation: run `zirc login`"
            ),
        }
    }
}

impl std::error::Error for Unpicked {}

/// "acme", "acme and globex", "acme, globex and initech".
fn and(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The organisations a command that names none may be for: the machine's own, or all of
/// them when it has none of its own.
#[must_use]
pub fn implied(orgs: &[Choice<'_>]) -> Vec<usize> {
    let own: Vec<usize> = (0..orgs.len()).filter(|&at| orgs[at].own).collect();
    if own.is_empty() {
        (0..orgs.len()).collect()
    } else {
        own
    }
}

/// Which of `orgs` a command acts on, by its index: the one `named` (its `--org`), else
/// `repo`'s, the organisation the repository it's run in belongs to, else the only one
/// [`implied`].
///
/// # Errors
///
/// [`Unpicked`] when `named` isn't one of them, or nothing says which of several.
pub fn pick(
    orgs: &[Choice<'_>],
    named: Option<&str>,
    repo: Option<&str>,
) -> Result<usize, Unpicked> {
    let listed = |at: &[usize]| at.iter().map(|&at| orgs[at].name.to_owned()).collect();
    let find = |name: &str| orgs.iter().position(|org| org.name == name);
    if let Some(name) = named {
        return find(name).ok_or_else(|| Unpicked::NotIn {
            name: name.to_owned(),
            orgs: listed(&(0..orgs.len()).collect::<Vec<_>>()),
        });
    }
    if let Some(at) = repo.and_then(find) {
        return Ok(at);
    }
    match implied(orgs).as_slice() {
        [] => Err(Unpicked::None),
        [at] => Ok(*at),
        several => Err(Unpicked::Several(listed(several))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACME: Choice<'static> = Choice {
        name: "acme",
        own: true,
    };
    const GLOBEX: Choice<'static> = Choice {
        name: "globex",
        own: true,
    };
    const GUEST: Choice<'static> = Choice {
        name: "demo",
        own: false,
    };
    const PUBLIC: Choice<'static> = Choice {
        name: "public",
        own: false,
    };

    /// What [`pick`] answers.
    type Picked = Result<usize, Unpicked>;

    /// What `pick` makes of each machine, with no --org and no repository, with acme's
    /// repository, and with --org demo.
    #[test]
    fn every_kind_of_machine_picks_as_the_rule_says() {
        let both = Unpicked::Several(vec!["acme".into(), "globex".into()]);
        let several: Picked = Err(both.clone());
        let cases: [(&[Choice<'_>], Picked, Picked); 6] = [
            // One organisation: needn't say.
            (&[ACME], Ok(0), Ok(0)),
            // Two: the repository says, or --org must.
            (&[ACME, GLOBEX], several.clone(), Ok(0)),
            (
                &[GLOBEX, ACME],
                Err(Unpicked::Several(vec!["globex".into(), "acme".into()])),
                Ok(1),
            ),
            // A guest's terminal alone, or the public server alone: that one.
            (&[GUEST], Ok(0), Ok(0)),
            // An own organisation beside a guest's terminal or the public server: the own.
            (&[GUEST, ACME], Ok(1), Ok(1)),
            (&[ACME, PUBLIC], Ok(0), Ok(0)),
        ];
        for (orgs, alone, in_repo) in cases {
            assert_eq!(pick(orgs, None, None), alone, "{orgs:?}");
            assert_eq!(
                pick(orgs, None, Some("acme")),
                in_repo,
                "{orgs:?} in acme's repository"
            );
        }
        // --org wins over the repository, and names a guest's terminal as well as any.
        assert_eq!(pick(&[ACME, GUEST], Some("demo"), Some("acme")), Ok(1));
        assert_eq!(
            pick(&[ACME, GLOBEX], Some("initech"), None)
                .unwrap_err()
                .to_string(),
            "this machine isn't signed in to initech, only to acme and globex"
        );
        assert_eq!(
            both.to_string(),
            "this machine is in acme and globex: say which with --org"
        );
        assert_eq!(pick(&[], None, None), Err(Unpicked::None));
        // Two guests' terminals and nothing of its own: both are implied, so it asks.
        assert!(matches!(
            pick(
                &[
                    GUEST,
                    Choice {
                        name: "other",
                        own: false
                    }
                ],
                None,
                None
            ),
            Err(Unpicked::Several(_))
        ));
    }
}
