//! Which organisations each command acts on ([`zirc_core::which_org`]), said once for every
//! command in [`ACTS`]: a new command fails the test until it's there, and one said to act on
//! one organisation fails it until it takes --org.

use std::path::Path;

use clap::{CommandFactory as _, Parser as _};

use crate::{Cli, run};

/// What a command acts on, on a machine signed in to several organisations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Acts {
    /// Every organisation at once, each line saying whose: it takes no --org.
    Every,
    /// One: the one --org names, else the repository's, else the only own one. It takes
    /// --org, which `zirc post` calls --in.
    One,
    /// The organisation of the repository it names or is run in, which always says.
    Repo,
    /// No organisation's conversations: this machine itself, or one it names some other way,
    /// such as `zirc orgs remove zirc`.
    Machine,
}

/// Every command, by its words, and what it acts on.
const ACTS: &[(&str, Acts)] = &[
    ("status", Acts::Every),
    ("watch", Acts::Every),
    ("orgs", Acts::Every),
    ("orgs list", Acts::Every),
    ("repo list", Acts::Every),
    ("post", Acts::One),
    ("dm", Acts::One),
    ("decide", Acts::One),
    ("decisions", Acts::One),
    ("request", Acts::One),
    ("wait-for", Acts::One),
    ("wait landed", Acts::One),
    ("wait deployed", Acts::One),
    ("wait state", Acts::One),
    ("stack", Acts::One),
    ("gotcha", Acts::Repo),
    ("gotchas", Acts::Repo),
    ("playbook", Acts::Repo),
    ("role take", Acts::Repo),
    ("role leave", Acts::Repo),
    ("role say", Acts::Repo),
    ("role last", Acts::Repo),
    ("note", Acts::One),
    ("settle", Acts::Repo),
    ("ask", Acts::One),
    ("reply", Acts::One),
    ("emit", Acts::One),
    ("card", Acts::One),
    ("task", Acts::One),
    ("wakes", Acts::One),
    ("wakes resume", Acts::One),
    ("thinking", Acts::One),
    ("join", Acts::One),
    ("leave", Acts::One),
    ("who", Acts::One),
    ("sessions", Acts::One),
    ("catchup", Acts::One),
    ("react", Acts::One),
    ("search", Acts::One),
    ("file", Acts::One),
    ("report", Acts::One),
    ("flag", Acts::One),
    ("block", Acts::One),
    ("unblock", Acts::One),
    ("blocked", Acts::One),
    ("lease", Acts::One),
    ("lease release", Acts::One),
    ("leases", Acts::One),
    ("team-key new", Acts::One),
    ("team-key add", Acts::Machine),
    ("team-key list", Acts::Every),
    ("team-key forget", Acts::One),
    ("unseal", Acts::One),
    ("state", Acts::One),
    ("state set", Acts::One),
    ("state clear", Acts::One),
    ("state take", Acts::One),
    ("trim", Acts::One),
    ("log", Acts::One),
    ("ci token", Acts::One),
    ("ci tokens", Acts::One),
    ("ci revoke", Acts::One),
    ("ci connect", Acts::One),
    ("ci test-failure", Acts::Repo),
    ("intend", Acts::Repo),
    ("clashes", Acts::Repo),
    ("repo add", Acts::Repo),
    ("repo remove", Acts::Repo),
    ("repo here", Acts::Repo),
    ("login", Acts::Machine),
    ("logout", Acts::Machine),
    ("orgs remove", Acts::Machine),
    ("init", Acts::Machine),
    ("agents queries", Acts::Machine),
    ("name", Acts::Machine),
    ("hints", Acts::Machine),
    ("hints check", Acts::Machine),
    ("hints add", Acts::Repo),
    ("hints remove", Acts::Repo),
    ("hints import", Acts::Repo),
    ("tui", Acts::Machine),
    ("tui login", Acts::Machine),
    ("tui logout", Acts::Machine),
    ("agent-guide", Acts::Machine),
    ("roles", Acts::Repo),
    ("avatars", Acts::Machine),
    ("reference", Acts::Machine),
    ("skill install", Acts::Machine),
    ("skill uninstall", Acts::Machine),
    ("daemon start", Acts::Machine),
    ("daemon stop", Acts::Machine),
    ("daemon restart", Acts::Machine),
    ("daemon install", Acts::Machine),
    ("daemon uninstall", Acts::Machine),
    ("update", Acts::Machine),
];

/// Every command that runs, by its words, and the flags it takes, its parents' included
/// where they may come before it.
fn commands() -> Vec<(String, Vec<String>)> {
    fn walk(
        command: &clap::Command,
        path: &str,
        inherited: &[String],
        into: &mut Vec<(String, Vec<String>)>,
    ) {
        let mut flags: Vec<String> = command
            .get_arguments()
            .filter_map(|arg| arg.get_long().map(str::to_owned))
            .collect();
        flags.extend(inherited.iter().cloned());
        let subs: Vec<_> = command
            .get_subcommands()
            .filter(|it| it.get_name() != "help")
            .collect();
        if !path.is_empty() && (subs.is_empty() || !command.is_subcommand_required_set()) {
            into.push((path.trim().to_owned(), flags.clone()));
        }
        let passed = if command.is_args_conflicts_with_subcommands_set() {
            Vec::new()
        } else {
            flags
        };
        for sub in subs {
            walk(sub, &format!("{path} {}", sub.get_name()), &passed, into);
        }
    }
    let mut found = Vec::new();
    walk(&Cli::command(), "", &[], &mut found);
    found
}

#[test]
fn every_command_says_which_organisations_it_acts_on() {
    let commands = commands();
    for (path, flags) in &commands {
        let Some((_, acts)) = ACTS.iter().find(|(it, _)| it == path) else {
            panic!(
                "zirc {path} isn't in which_org::ACTS: say whether it acts on every \
                 organisation, one, its repository's or none"
            );
        };
        // post's --org is its topics'; it names an organisation with --in.
        let names_one = flags.iter().any(|flag| flag == "org")
            || (path == "post" && flags.iter().any(|flag| flag == "in"));
        match acts {
            Acts::One => assert!(names_one, "zirc {path} acts on one: it needs --org"),
            Acts::Every => assert!(!names_one, "zirc {path} acts on every: no --org"),
            Acts::Repo | Acts::Machine => {}
        }
    }
    for (path, _) in ACTS {
        assert!(
            commands.iter().any(|(it, _)| it == path),
            "which_org::ACTS has zirc {path}, which isn't a command"
        );
    }
}

/// A machine signed in to zirc and globex through zirc.dev, whose zirc.dev and zircd don't
/// answer: nothing's read before the organisation is picked.
fn two_organisations(home: &Path) {
    std::fs::write(
        zirc_core::paths::config(home),
        "web = \"http://127.0.0.1:9\"\n[server]\nhost = \"zirc.irc.zirc.dev\"\n\
         [account]\nname = \"ada-mbp\"\npassword = \"pw\"\n\
         [[org]]\nname = \"globex\"\nweb = \"http://127.0.0.1:9\"\n\
         [org.server]\nhost = \"globex.irc.zirc.dev\"\n\
         [org.account]\nname = \"ada-mbp\"\npassword = \"pw\"\n",
    )
    .unwrap();
}

#[tokio::test]
async fn reading_one_organisation_on_a_machine_in_two_asks_which() {
    let home = tempfile::tempdir().unwrap();
    two_organisations(home.path());
    for words in [
        &["search", "timeout"][..],
        &["catchup", "--team"],
        &["log"],
        &["blocked"],
    ] {
        let cli = Cli {
            home: Some(home.path().to_owned()),
            ..Cli::parse_from(std::iter::once("zirc").chain(words.iter().copied()))
        };
        let err = run(cli, &mut Vec::new()).await.unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            "this machine is in zirc and globex: say which with --org",
            "zirc {}",
            words.join(" ")
        );
    }
}
