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
    assert_eq!(display_name("zirc"), "Zirc");
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
        added_toast("fjord", &SwitchHint::Rail(Some("⌘2".into()))),
        "Added Fjord. Switch with the rail or ⌘2."
    );
    assert_eq!(
        added_toast("fjord", &SwitchHint::Menu("zirc".into())),
        "Added Fjord. Switch from the Zirc ▾ menu."
    );
    assert_eq!(
        renamed_toast("zirc", "sam.k"),
        "Your name in Zirc is now @sam.k"
    );
}

#[test]
fn the_sign_out_sheets() {
    let (_dir, account) = account(&["zirc.irc.zirc.dev", "fjord.irc.zirc.dev"]);
    let one = account.sign_out_text("zirc").unwrap();
    assert_eq!(one.title, "Sign out of Zirc?");
    assert_eq!(
        one.body,
        "Zirc's messages, notifications and settings leave this device. You stay signed in to Fjord."
    );
    assert_eq!(one.confirm, "Sign out of Zirc");
    assert_eq!(account.sign_out_text("nope"), None);
    let all = account.sign_out_everything_text();
    assert_eq!(all.title, "Sign out of everything?");
    assert_eq!(
        all.body,
        "Signs you out of Zirc, Fjord and zirc.dev on this device."
    );
    assert_eq!(all.confirm, "Sign out of everything");

    let (_dir, last) = account_one("zirc.irc.zirc.dev");
    let sheet = last.sign_out_text("zirc").unwrap();
    assert_eq!(
        sheet.body,
        "Zirc is the only organisation on this device, so this also signs you out of zirc.dev here."
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
    let (_dir, one) = account_one("zirc.irc.zirc.dev");
    assert_eq!(one.added_text("zirc", &SwitchHint::Rail(None)), None);
    let (_dir, two) = account(&["zirc.irc.zirc.dev", "fjord.irc.zirc.dev"]);
    assert_eq!(
        two.added_text("fjord", &SwitchHint::Rail(None)).as_deref(),
        Some("Added Fjord. Switch with the rail.")
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
    assert_eq!(card(OrgState::Offline, "zirc"), None);
    let ended = card(OrgState::AccessEnded, "zirc").unwrap();
    assert_eq!(ended.title, "You're no longer in Zirc");
    assert_eq!(ended.actions, [CardAction::Remove]);
    // Its button says what it does, as the organisation's own sign-out item does.
    assert_eq!(ended.label(CardAction::Remove), "Sign out of Zirc");
    let again = card(OrgState::SignInAgain, "zirc").unwrap();
    assert_eq!(again.label(again.actions[0]), "Sign in again");
    assert_eq!(thread_id("zirc", "#general"), "zirc/#general");
    let summary = OrgSummary {
        org: "zirc".into(),
        host: "zirc.irc.zirc.dev".into(),
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
    assert_eq!(summary.a11y_label(), "Zirc, 3 mentions, connecting");
    assert_eq!(summary.line(), "@sam · Connecting…");
    // On a server known only by its address, the name its header shows: "A", never "1".
    let numeric = OrgSummary {
        org: "127.0.0.1".into(),
        host: "127.0.0.1".into(),
        self_hosted: true,
        title: Some("Zirc".into()),
        ..summary.clone()
    };
    assert_eq!(numeric.name(), "Zirc");
    assert_eq!(numeric.initials(), "Z");
    assert_eq!(numeric.card(), None);
    let ended = OrgSummary {
        state: OrgState::AccessEnded,
        ..numeric.clone()
    };
    assert_eq!(ended.card().unwrap().title, "You're no longer in Zirc");
    // Before its header is known, the address is the last resort.
    let untitled = OrgSummary {
        title: None,
        ..numeric
    };
    assert_eq!(untitled.name(), "127.0.0.1");
}

#[test]
fn says_what_was_signed_out_of_by_its_name() {
    assert_eq!(signed_out("zirc"), "Signed out of Zirc.");
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
    let mut zirc = Joinable {
        slug: "zirc".into(),
        name: "Zirc Inc".into(),
        host: "zirc.irc.zirc.dev".into(),
        handle: "sam".into(),
        ready: true,
        web: "https://zirc.dev".into(),
    };
    assert_eq!(zirc.label(), "Join Zirc Inc");
    assert_eq!(zirc.line(), "@sam");
    assert_eq!(zirc.a11y_label(), "Join Zirc Inc, as @sam");
    assert_eq!(zirc.initials(), org_initials("Zirc Inc"));
    // The same colour as once joined, when it is named by its host.
    assert_eq!(zirc.colour(), org_colour("zirc"));
    assert_eq!(zirc.failed(&Error::Add(AddError::TooMany(8))), too_many(8));
    zirc.ready = false;
    zirc.name = String::new();
    assert_eq!(zirc.label(), "Join Zirc");
    assert_eq!(zirc.line(), "@sam · Getting ready");
    assert_eq!(zirc.a11y_label(), "Join Zirc, getting ready");
    assert!(
        zirc.failed(&Error::NotJoinable("zirc".into()))
            .contains("isn't ready")
    );
}
