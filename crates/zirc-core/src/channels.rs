//! Which channels to join and leave. [`reconcile`] compares the channels a connection is
//! in with the ones it should be in, so the daemon only has to carry out the difference.

use std::collections::BTreeMap;

/// The joins and parts that take a connection from one set of channels to another.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Channels to join, sorted.
    pub join: Vec<String>,
    /// Channels to leave, sorted.
    pub part: Vec<String>,
}

impl Plan {
    /// Whether the connection is already where it should be.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.join.is_empty() && self.part.is_empty()
    }
}

/// What it takes to go from the `joined` channels to the `desired` ones.
///
/// IRC compares channel names without case, so `#API` and `#api` are one channel. Names
/// are kept as the side they came from spells them.
#[must_use]
pub fn reconcile<'a>(
    joined: impl IntoIterator<Item = &'a str>,
    desired: impl IntoIterator<Item = &'a str>,
) -> Plan {
    let joined = by_folded_name(joined);
    let desired = by_folded_name(desired);
    Plan {
        join: missing_from(&desired, &joined),
        part: missing_from(&joined, &desired),
    }
}

fn by_folded_name<'a>(names: impl IntoIterator<Item = &'a str>) -> BTreeMap<String, &'a str> {
    names
        .into_iter()
        .map(|name| (name.to_lowercase(), name))
        .collect()
}

fn missing_from(names: &BTreeMap<String, &str>, other: &BTreeMap<String, &str>) -> Vec<String> {
    names
        .iter()
        .filter(|(folded, _)| !other.contains_key(*folded))
        .map(|(_, name)| (*name).to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_and_parts_the_difference() {
        let plan = reconcile(["#api", "#old"], ["#api", "#api/bots", "#zirc"]);
        assert_eq!(plan.join, ["#api/bots", "#zirc"]);
        assert_eq!(plan.part, ["#old"]);
    }

    #[test]
    fn nothing_to_do_when_already_there() {
        assert!(reconcile(["#a", "#b"], ["#b", "#a"]).is_empty());
        assert!(reconcile([], []).is_empty());
    }

    #[test]
    fn ignores_case() {
        assert!(reconcile(["#API"], ["#api"]).is_empty());
    }
}
