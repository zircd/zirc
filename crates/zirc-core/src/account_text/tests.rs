use super::*;
use crate::account::{OrgSession, Policy};
use crate::secrets::{File, Secrets as _};
use crate::signin::SignIn;
use std::sync::Arc;
use zirc_chat::chat::{Chat, Options};
use zirc_chat::orgs::ORG_PALETTE;
use zirc_proto::login::Server;

fn account(hosts: &[&str]) -> (tempfile::TempDir, Account) {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(File(dir.path().join("secrets.toml")));
    let sessions: Vec<_> = hosts
        .iter()
        .map(|host| {
            let signin = SignIn {
                handle: "sam".into(),
                nick: "sam".into(),
                account: true,
                web: "https://zirc.dev".into(),
                server: Server {
                    host: (*host).into(),
                    port: None,
                    tls: None,
                    password: None,
                },
                realname: None,
                device_key: false,
                media: None,
            };
            secrets.set(&signin.key(), "pass").unwrap();
            let chat = Chat::new("sam", host, Vec::new(), Options::default());
            OrgSession::new(signin, chat)
        })
        .collect();
    let account = Account::open(secrets, Policy::default().with_several(true), sessions).unwrap();
    (dir, account)
}

#[test]
fn names() {
    assert_eq!(display_name("acme"), "Acme");
    assert_eq!(display_name("zirc-labs"), "Zirc Labs");
    assert_eq!(display_name("chat.example.org"), "chat.example.org");
}

#[test]
fn the_words_of_the_design() {
    assert_eq!(
        too_many(8),
        "You can have up to 8 organisations on this device. Sign out of one to add another."
    );
    assert_eq!(
        added_toast("zirc", &SwitchHint::Rail(Some("⌘2".into()))),
        "Added Zirc. Switch with the rail or ⌘2."
    );
    assert_eq!(
        added_toast("zirc", &SwitchHint::Menu("acme".into())),
        "Added Zirc. Switch from the Acme ▾ menu."
    );
    assert_eq!(
        renamed_toast("acme", "sam.k"),
        "Your name in Acme is now @sam.k"
    );
}

#[test]
fn the_sign_out_sheets() {
    let (_dir, account) = account(&["acme.irc.zirc.dev", "zirc.irc.zirc.dev"]);
    let one = account.sign_out_text("acme").unwrap();
    assert_eq!(one.title, "Sign out of Acme?");
    assert_eq!(
        one.body,
        "Acme's messages, notifications and settings leave this device. You stay signed in to Zirc."
    );
    assert_eq!(one.confirm, "Sign out of Acme");
    assert_eq!(account.sign_out_text("nope"), None);
    let all = account.sign_out_everything_text();
    assert_eq!(all.title, "Sign out of everything?");
    assert_eq!(
        all.body,
        "Signs you out of Acme, Zirc and zirc.dev on this device."
    );
    assert_eq!(all.confirm, "Sign out of everything");

    let (_dir, last) = account_one("acme.irc.zirc.dev");
    let sheet = last.sign_out_text("acme").unwrap();
    assert_eq!(
        sheet.body,
        "Acme is the only organisation on this device, so this also signs you out of zirc.dev here."
    );
    assert_eq!(sheet.confirm, "Sign out");
    // A self-hosted last organisation has no zirc.dev to sign out of.
    let (_dir, own) = account_one("chat.example.org");
    assert_eq!(
        own.sign_out_text("chat.example.org").unwrap().body,
        "chat.example.org is the only organisation on this device."
    );
}

fn account_one(host: &str) -> (tempfile::TempDir, Account) {
    account(&[host])
}

#[test]
fn everything_lists_three_names_then_how_many_more() {
    let hosts: Vec<String> = (0..5).map(|n| format!("o{n}.irc.zirc.dev")).collect();
    let refs: Vec<&str> = hosts.iter().map(String::as_str).collect();
    let (_dir, account) = account(&refs);
    assert_eq!(
        account.sign_out_everything_text().body,
        "Signs you out of O0, O1, O2, 2 more and zirc.dev on this device."
    );
}

#[test]
fn the_toast_after_adding_is_for_the_second_organisation_on() {
    let (_dir, one) = account_one("acme.irc.zirc.dev");
    assert_eq!(one.added_text("acme", &SwitchHint::Rail(None)), None);
    let (_dir, two) = account(&["acme.irc.zirc.dev", "zirc.irc.zirc.dev"]);
    assert_eq!(
        two.added_text("zirc", &SwitchHint::Rail(None)).as_deref(),
        Some("Added Zirc. Switch with the rail.")
    );
}

#[test]
fn a_summary_has_its_tile() {
    let (_dir, account) = account(&["zirc-labs.irc.zirc.dev"]);
    let summary = &account.organisations()[0];
    assert_eq!(summary.name(), "Zirc Labs");
    assert_eq!(summary.initials(), "ZL");
    assert!(usize::from(summary.colour()) < ORG_PALETTE.len());
}

#[test]
fn says_each_state_its_card_and_its_tile() {
    assert_eq!(card(OrgState::Offline, "acme"), None);
    let ended = card(OrgState::AccessEnded, "acme").unwrap();
    assert_eq!(ended.title, "You're no longer in Acme");
    assert_eq!(ended.actions, [CardAction::Remove]);
    // Its button says what it does, as the organisation's own sign-out item does.
    assert_eq!(ended.label(CardAction::Remove), "Sign out of Acme");
    let again = card(OrgState::SignInAgain, "acme").unwrap();
    assert_eq!(again.label(again.actions[0]), "Sign in again");
    assert_eq!(thread_id("acme", "#general"), "acme/#general");
    let summary = OrgSummary {
        org: "acme".into(),
        host: "acme.irc.zirc.dev".into(),
        handle: "sam".into(),
        state: OrgState::Connecting,
        unread: zirc_chat::orgs::Unread {
            unread: 5,
            mentions: 3,
        },
        self_hosted: false,
        selected: false,
        title: None,
    };
    assert_eq!(summary.a11y_label(), "Acme, 3 mentions, connecting");
    assert_eq!(summary.line(), "@sam · Connecting…");
    // On a server known only by its address, the name its header shows: "A", never "1".
    let numeric = OrgSummary {
        org: "127.0.0.1".into(),
        host: "127.0.0.1".into(),
        self_hosted: true,
        title: Some("Acme".into()),
        ..summary.clone()
    };
    assert_eq!(numeric.name(), "Acme");
    assert_eq!(numeric.initials(), "A");
    assert_eq!(numeric.card(), None);
    let ended = OrgSummary {
        state: OrgState::AccessEnded,
        ..numeric.clone()
    };
    assert_eq!(ended.card().unwrap().title, "You're no longer in Acme");
    // Before its header is known, the address is the last resort.
    let untitled = OrgSummary {
        title: None,
        ..numeric
    };
    assert_eq!(untitled.name(), "127.0.0.1");
}

#[test]
fn says_what_was_signed_out_of_by_its_name() {
    assert_eq!(signed_out("acme"), "Signed out of Acme.");
    assert_eq!(signed_out("zirc-labs"), "Signed out of Zirc Labs.");
    assert_eq!(
        signed_out("irc.example.com"),
        "Signed out of irc.example.com."
    );
}

#[test]
fn a_joinable_organisation_says_join_and_whether_it_is_ready() {
    use crate::account::AddError;
    use crate::account_keys::{Error, Joinable};
    let mut acme = Joinable {
        slug: "acme".into(),
        name: "Acme Inc".into(),
        host: "acme.irc.zirc.dev".into(),
        handle: "sam".into(),
        ready: true,
        web: "https://zirc.dev".into(),
    };
    assert_eq!(acme.label(), "Join Acme Inc");
    assert_eq!(acme.line(), "@sam");
    assert_eq!(acme.a11y_label(), "Join Acme Inc, as @sam");
    assert_eq!(acme.initials(), org_initials("Acme Inc"));
    // The same colour as once joined, when it is named by its host.
    assert_eq!(acme.colour(), org_colour("acme"));
    assert_eq!(acme.failed(&Error::Add(AddError::TooMany(8))), too_many(8));
    acme.ready = false;
    acme.name = String::new();
    assert_eq!(acme.label(), "Join Acme");
    assert_eq!(acme.line(), "@sam · Getting ready");
    assert_eq!(acme.a11y_label(), "Join Acme, getting ready");
    assert!(
        acme.failed(&Error::NotJoinable("acme".into()))
            .contains("isn't ready")
    );
}
