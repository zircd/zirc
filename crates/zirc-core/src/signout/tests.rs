use super::*;
use crate::settings::Settings;

fn settings(text: &str) -> Settings {
    Settings::from_toml(text).unwrap()
}

const TWO: &str = r#"
nick = "m"
web = "https://zirc.dev"
[server]
host = "acme.irc.zirc.dev"
[account]
name = "m"
password = "p"
[[org]]
web = "https://zirc.dev"
[org.server]
host = "globex.irc.zirc.dev"
[org.account]
name = "m"
password = "q"
"#;

const GUEST: &str = r##"
[server]
host = "acme.irc.zirc.dev"
[guest]
url = "wss://acme.irc.zirc.dev/guest"
org = "acme"
guest = "guest-1"
channel = "#web/guest-1"
public = "#web"
grant = "g"
expires = 9
"##;

#[test]
fn one_of_two_keeps_the_other_and_zirc_dev() {
    let plan = Plan::new(&settings(TWO), &Scope::One("acme".into())).unwrap();
    assert!(!plan.ends_zirc_dev());
    assert!(!plan.leaves_nothing());
    let sheet = plan.sheet();
    assert_eq!(sheet.title, "Sign out of Acme?");
    assert!(
        sheet.body.contains("You stay signed in to Globex."),
        "{sheet:?}"
    );
}

#[test]
fn the_last_one_signs_out_of_zirc_dev_too() {
    let mut s = settings(TWO);
    assert!(s.take_organisation("acme"));
    let plan = Plan::new(&s, &Scope::One("globex".into())).unwrap();
    assert!(plan.ends_zirc_dev());
    assert!(plan.leaves_nothing());
    assert!(plan.sheet().body.contains("also signs you out of zirc.dev"));
}

#[test]
fn everything_names_them_and_zirc_dev() {
    let plan = Plan::new(&settings(TWO), &Scope::Everything).unwrap();
    assert!(plan.ends_zirc_dev());
    let sheet = plan.sheet();
    assert_eq!(sheet.title, "Sign out of everything?");
    assert_eq!(
        sheet.body,
        "Signs you out of Acme, Globex and zirc.dev on this device."
    );
}

#[test]
fn a_guests_terminal_has_no_zirc_dev_to_leave() {
    let plan = Plan::new(&settings(GUEST), &Scope::One("acme".into())).unwrap();
    assert!(!plan.ends_zirc_dev());
    assert!(plan.leaves_nothing());
    assert!(!plan.sheet().body.contains("zirc.dev"));
}

#[test]
fn an_unknown_organisation_is_refused() {
    let refused = Plan::new(&settings(TWO), &Scope::One("initech".into())).unwrap_err();
    assert_eq!(
        refused.to_string(),
        "this machine isn't signed in to initech"
    );
}

#[test]
fn taking_the_first_moves_the_next_up() {
    let mut s = settings(TWO);
    assert!(s.take_organisation("acme"));
    assert_eq!(s.server.host, "globex.irc.zirc.dev");
    assert!(s.orgs.is_empty());
    assert!(!s.take_organisation("globex"), "the only one stays");
}
