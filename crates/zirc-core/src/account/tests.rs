use super::*;
use crate::secrets::File;
use zirc_chat::chat::{Level, Options};
use zirc_proto::login::Server;

fn signin_at(handle: &str, host: &str, web: &str, device_key: bool) -> SignIn {
    SignIn {
        handle: handle.into(),
        nick: handle.into(),
        account: true,
        web: web.into(),
        server: Server {
            host: host.into(),
            port: None,
            tls: None,
            password: None,
        },
        realname: None,
        device_key,
        media: None,
    }
}

fn acme() -> SignIn {
    signin_at("alice", "acme.irc.zirc.dev", "https://zirc.dev", true)
}

fn zirc() -> SignIn {
    signin_at("ali", "zirc.irc.zirc.dev", "https://zirc.dev", false)
}

fn session(signin: SignIn) -> OrgSession {
    let chat = Chat::new(
        &signin.nick,
        &signin.server.host,
        Vec::new(),
        Options::default(),
    );
    OrgSession::new(signin, chat)
}

struct Home {
    dir: tempfile::TempDir,
    secrets: Arc<File>,
}

fn home() -> Home {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(File(dir.path().join("secrets.toml")));
    Home { dir, secrets }
}

fn several() -> Policy {
    Policy::default().with_several(true)
}

fn account_with(home: &Home, policy: Policy, signins: &[SignIn]) -> Account {
    for signin in signins {
        home.secrets
            .set(&signin.key(), &format!("{}'s passphrase", signin.handle))
            .unwrap();
    }
    Account::open(
        home.secrets.clone(),
        policy,
        signins.iter().cloned().map(session),
    )
    .unwrap()
}

#[test]
fn migrates_the_unkeyed_data_of_a_single_organisation_without_signing_in_again() {
    let home = home();
    for (name, value) in [
        (keys::NOTIFY, r##"{"levels":{"#api":"all"}}"##),
        (keys::BADGE, "7"),
        (keys::PUSH_NICK, "alice"),
    ] {
        home.secrets.set(name, value).unwrap();
    }
    // The old file: the lone sign-in, as older releases wrote it.
    let file = home.dir.path().join("signin.toml");
    SignIns::from(acme()).save(&file).unwrap();
    let signins = SignIns::load(&file).unwrap();
    let account = account_with(
        &home,
        Policy::default(),
        &signins.iter().cloned().collect::<Vec<_>>(),
    );
    let (org, acme) = account.selected().unwrap();
    assert_eq!(org, "acme");
    assert_eq!(acme.badge, 7);
    assert_eq!(acme.chat().level("#api"), Level::All);
    // Under the organisation, never the handle.
    assert_eq!(home.secrets.get("zirc-badge:acme").as_deref(), Some("7"));
    assert_eq!(home.secrets.get("zirc-badge:alice"), None);
    // The passphrase stays where it was.
    assert!(home.secrets.get("alice@acme.irc.zirc.dev").is_some());
    // The iOS extension's unkeyed names are still there while the switch is off.
    assert_eq!(home.secrets.get("zirc-badge").as_deref(), Some("7"));
}

#[test]
fn migrates_from_each_older_shape_of_the_sign_in_file() {
    let json_list = serde_json::to_string(&[acme(), zirc()]).unwrap();
    let json_one = serde_json::to_string(&acme()).unwrap();
    let shapes: [(&str, usize); 2] = [(&json_one, 1), (&json_list, 2)];
    for (json, count) in shapes {
        let signins: SignIns = serde_json::from_str(json).unwrap();
        assert_eq!(signins.len(), count);
        let home = home();
        home.secrets.set(keys::BADGE, "3").unwrap();
        let account = account_with(
            &home,
            several(),
            &signins.iter().cloned().collect::<Vec<_>>(),
        );
        // The data moves under the first organisation and no other, and the unkeyed name goes
        // once the switch is on.
        assert_eq!(account.get("acme").unwrap().badge, 3);
        if count == 2 {
            assert_eq!(account.get("zirc").unwrap().badge, 0);
        }
        assert_eq!(home.secrets.get("zirc-badge"), None);
    }
    // The TOML shapes: the lone sign-in and `[[signin]]`.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.toml");
    let two: SignIns = serde_json::from_str(&json_list).unwrap();
    two.save(&path).unwrap();
    assert_eq!(SignIns::load(&path).unwrap().len(), 2);
}

#[test]
fn migrating_twice_changes_nothing() {
    let home = home();
    home.secrets.set(keys::BADGE, "3").unwrap();
    let account = account_with(&home, Policy::default(), &[acme()]);
    drop(account);
    home.secrets.set(keys::BADGE, "9").unwrap();
    let account = account_with(&home, Policy::default(), &[acme()]);
    // The organisation's own value wins over a stale unkeyed one.
    assert_eq!(account.get("acme").unwrap().badge, 3);
}

#[test]
fn the_rollout_switch_off_keeps_one_organisation() {
    let home = home();
    let mut account = account_with(&home, Policy::default(), &[acme()]);
    assert!(!account.can_add());
    assert_eq!(
        account.add_organisation(session(zirc())),
        Err(AddError::NotEnabled)
    );
    assert_eq!(account.len(), 1);
    // Signing in to the same organisation again is not adding.
    assert_eq!(account.add_organisation(session(acme())), Ok(()));
    account.set_several(true);
    assert!(account.can_add());
    assert_eq!(account.add_organisation(session(zirc())), Ok(()));
    assert_eq!(
        account
            .organisations()
            .iter()
            .map(|o| o.org.as_str())
            .collect::<Vec<_>>(),
        ["acme", "zirc"]
    );
}

#[test]
fn holds_at_most_eight_organisations() {
    let home = home();
    let mut account = account_with(&home, several(), &[]);
    for n in 0..MAX_ORGANISATIONS {
        let host = format!("org{n}.irc.zirc.dev");
        assert_eq!(
            account.add_organisation(session(signin_at(
                "alice",
                &host,
                "https://zirc.dev",
                false
            ))),
            Ok(())
        );
    }
    assert_eq!(
        account.add_organisation(session(zirc())),
        Err(AddError::TooMany(8))
    );
    assert!(!account.can_add());
}

#[test]
fn a_handshake_with_several_organisations_merges_and_replaces_none() {
    let home = home();
    let mut account = account_with(&home, several(), &[acme()]);
    account.get_mut("acme").unwrap().badge = 4;
    let merged: SignIns =
        serde_json::from_str(&serde_json::to_string(&[acme(), zirc()]).unwrap()).unwrap();
    let refused = account.add_signins(&merged, |s| session(s.clone()));
    assert!(refused.is_empty());
    assert_eq!(account.len(), 2);
    assert_eq!(account.selected().unwrap().0, "acme");
    assert_eq!(account.signins().len(), 2);
    assert_eq!(
        account.connected_hosts(),
        ["acme.irc.zirc.dev", "zirc.irc.zirc.dev"]
    );
}

#[test]
fn signing_out_of_one_leaves_the_others_and_keeps_zirc_dev() {
    let home = home();
    let mut account = account_with(&home, several(), &[acme(), zirc()]);
    let store = account.store().clone();
    for org in ["acme", "zirc"] {
        let s = account.get_mut(org).unwrap();
        s.badge = 2;
        s.blocked.blocked.clear();
        s.push = PushEndpoint {
            token: Some("tok".into()),
            registered: true,
            registered_at: None,
            endpoint_id: Some("ep1".into()),
        };
        s.save(&store).unwrap();
    }
    assert_eq!(account.org_for_endpoint("ep1"), Some("acme"));
    assert_eq!(account.org_for_endpoint("nope"), None);
    let key = |org: &str| crate::sealed::Keys::name(org);
    for name in [
        key("acme"),
        key("zirc"),
        crate::sealed::DEVICE_SECRET.to_owned(),
    ] {
        home.secrets.set(&name, "kept").unwrap();
    }
    let removal = account.remove_organisation("acme").unwrap().unwrap();
    assert_eq!(removal.signin.handle, "alice");
    assert!(removal.push.registered && removal.unregister_device_key);
    assert_eq!(removal.push.endpoint_id.as_deref(), Some("ep1"));
    assert!(!removal.last && !removal.ends_zirc_dev());
    // Acme's passphrase and data are gone, Zirc's are not.
    assert_eq!(home.secrets.get("alice@acme.irc.zirc.dev"), None);
    assert_eq!(home.secrets.get("zirc-badge:acme"), None);
    assert_eq!(home.secrets.get("zirc-push-nick:acme"), None);
    assert!(home.secrets.get("ali@zirc.irc.zirc.dev").is_some());
    assert_eq!(home.secrets.get("zirc-badge:zirc").as_deref(), Some("2"));
    // Acme's sealing keys went with it; Zirc's, and the device's own, stay.
    assert_eq!(home.secrets.get(&key("acme")), None);
    assert!(home.secrets.get(&key("zirc")).is_some());
    assert!(home.secrets.get(crate::sealed::DEVICE_SECRET).is_some());
    // Zirc is shown now.
    assert_eq!(account.selected().unwrap().0, "zirc");
    assert_eq!(account.remove_organisation("acme").unwrap(), None);
    // Zirc's registration of the device key was never made.
    let last = account.remove_organisation("zirc").unwrap().unwrap();
    assert!(!last.unregister_device_key);
    assert!(last.last && last.ends_zirc_dev());
    assert!(account.is_empty());
    // The last one takes the device's key pair with it.
    assert_eq!(home.secrets.get(&key("zirc")), None);
    assert_eq!(home.secrets.get(crate::sealed::DEVICE_SECRET), None);
}

#[test]
fn the_last_self_hosted_organisation_has_no_zirc_dev_to_sign_out_of() {
    let home = home();
    let own = signin_at(
        "alice",
        "chat.example.org",
        "https://chat.example.org",
        false,
    );
    let mut account = account_with(&home, Policy::default(), &[own]);
    assert!(account.get("chat.example.org").unwrap().self_hosted());
    let removal = account
        .remove_organisation("chat.example.org")
        .unwrap()
        .unwrap();
    assert!(removal.last && !removal.ends_zirc_dev());
}

#[test]
fn signing_out_of_everything_says_what_else_goes() {
    let home = home();
    let mut account = account_with(&home, several(), &[acme(), zirc()]);
    let out = account.sign_out_everything().unwrap();
    assert_eq!(out.removed.len(), 2);
    assert!(out.delete_push_keys && out.delete_device_key && out.ends_zirc_dev());
    assert!(account.is_empty());
    assert_eq!(home.secrets.get("alice@acme.irc.zirc.dev"), None);
    assert_eq!(home.secrets.get("ali@zirc.irc.zirc.dev"), None);
}

#[test]
fn data_follows_the_organisation_through_a_rename() {
    let home = home();
    let mut account = account_with(&home, several(), &[acme()]);
    account.get_mut("acme").unwrap().badge = 5;
    let store = account.store().clone();
    account.get("acme").unwrap().save(&store).unwrap();
    let renamed = acme().renamed("alicia", home.secrets.as_ref()).unwrap();
    account.add_organisation(session(renamed)).unwrap();
    // The same organisation, so the same data and nothing lost.
    assert_eq!(account.len(), 1);
    assert_eq!(account.get("acme").unwrap().badge, 5);
    assert_eq!(account.get("acme").unwrap().signin().handle, "alicia");
}

#[test]
fn a_muted_organisation_counts_on_its_tile_but_not_on_the_icon() {
    let home = home();
    let mut account = account_with(&home, several(), &[acme(), zirc()]);
    assert_eq!(account.badges().total, Unread::default());
    assert_eq!(account.badges().orgs.len(), 2);
    // Pushes brought news to both while the app was closed.
    account.get_mut("acme").unwrap().pushed = 2;
    account.get_mut("zirc").unwrap().pushed = 3;
    assert_eq!(account.icon_badge(None), 5);
    // Muted: its tile still counts, in grey; the icon leaves it out.
    account.get_mut("zirc").unwrap().muted = true;
    assert_eq!(account.badges().orgs[1].1.mentions, 3);
    assert_eq!(account.icon_badge(None), 2);
    assert_eq!(account.title("acme", "#api"), "acme · #api");
    let one = account_with(&self::home(), Policy::default(), &[acme()]);
    assert_eq!(one.title("acme", "#api"), "#api");
}

#[test]
fn a_phone_connects_only_the_organisation_shown() {
    let home = home();
    let hosts: Vec<_> = (0..3)
        .map(|n| {
            signin_at(
                "alice",
                &format!("o{n}.irc.zirc.dev"),
                "https://zirc.dev",
                false,
            )
        })
        .collect();
    let mut phone = account_with(&home, Policy::phone().with_several(true), &hosts);
    assert_eq!(phone.live(true), ["o0"]);
    phone.select("o2");
    assert_eq!(phone.live(true), ["o2"]);
    assert!(phone.live(false).is_empty());
    let home = self::home();
    let mut desktop = account_with(&home, several(), &hosts);
    desktop.select("o1");
    assert_eq!(desktop.live(true), ["o1", "o0", "o2"]);
    let s = desktop.get_mut("o0").unwrap();
    let generation = s.reconnect();
    s.set_state(generation, OrgState::AccessEnded);
    assert_eq!(desktop.live(true), ["o1", "o2"]);
}

#[test]
fn an_organisations_push_registration_is_kept_with_when() {
    let home = home();
    let hosts: Vec<_> = (0..4)
        .map(|n| {
            signin_at(
                "alice",
                &format!("o{n}.irc.zirc.dev"),
                "https://zirc.dev",
                false,
            )
        })
        .collect();
    let mut phone = account_with(&home, Policy::phone().with_several(true), &hosts);
    let now = 10_000_000;
    phone.get_mut("o1").unwrap().registered(now - 60);
    phone.get_mut("o2").unwrap().registered(now - 86_400);
    let store = phone.store().clone();
    phone.get("o2").unwrap().save(&store).unwrap();
    assert_eq!(
        home.secrets.get("zirc-push-registered:o2").as_deref(),
        Some((now - 86_400).to_string().as_str())
    );
}

#[test]
fn an_unconnected_organisations_unread_is_the_pushes_not_yet_read() {
    let home = home();
    let mut account = account_with(&home, Policy::phone().with_several(true), &[acme(), zirc()]);
    let org = account.get_mut("zirc").unwrap();
    for read in [false, false, true, true, true] {
        org.pushed_one(read);
    }
    assert_eq!(org.pushed, 0);
    org.pushed_one(false);
    assert_eq!(account.badges().total.mentions, 1);
    // Kept across starts; opening it recounts from the server's chat (nothing unread here).
    let store = account.store().clone();
    account.get("zirc").unwrap().save(&store).unwrap();
    let account = account_with(&home, Policy::phone().with_several(true), &[acme(), zirc()]);
    assert_eq!(account.get("zirc").unwrap().pushed, 1);
    let mut account = account;
    account.get_mut("zirc").unwrap().recounted();
    assert_eq!(account.get("zirc").unwrap().pushed, 0);
}

#[test]
fn an_old_connections_word_is_ignored() {
    let mut s = session(acme());
    let old = s.reconnect();
    let new = s.reconnect();
    s.set_state(old, OrgState::Connected);
    assert_eq!(s.state(), OrgState::Connecting);
    s.set_state(new, OrgState::Connected);
    assert_eq!(s.state(), OrgState::Connected);
}

/// `bytes` as base64, for the Authorization header.
fn base64(bytes: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, b| n << 8 | u32::from(*b)) << (8 * (3 - chunk.len()));
        for i in 0..=chunk.len() {
            out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
        out.extend(std::iter::repeat_n('=', 3 - chunk.len()));
    }
    out
}

#[tokio::test]
async fn each_organisations_request_carries_its_own_passphrase_to_its_own_web() {
    let (web_a, served_a) = zirc_test_ergo::web::serve(vec![(401, String::new())]);
    let (web_b, served_b) = zirc_test_ergo::web::serve(vec![(401, String::new())]);
    // Two organisations on different hostnames, the same handle, different passphrases.
    let a = signin_at("alice", "acme.irc.zirc.dev", &web_a, false);
    let b = signin_at("alice", "chat.example.org", &web_b, false);
    let home = home();
    home.secrets.set(&a.key(), "passphrase-for-acme").unwrap();
    home.secrets
        .set(&b.key(), "passphrase-for-example")
        .unwrap();
    let account = Account::open(home.secrets.clone(), several(), [session(a), session(b)]).unwrap();
    for org in ["acme", "chat.example.org"] {
        let call = account
            .get(org)
            .unwrap()
            .call(home.secrets.as_ref())
            .unwrap();
        assert!(
            call.client
                .person(&call.credential)
                .await
                .unwrap()
                .is_none()
        );
    }
    let (got_a, got_b) = (served_a.join().unwrap(), served_b.join().unwrap());
    let header = |secret: &str| format!("Basic {}", base64(format!("alice:{secret}").as_bytes()));
    assert!(
        got_a[0].contains(&header("passphrase-for-acme")),
        "{}",
        got_a[0]
    );
    assert!(!got_a[0].contains(&header("passphrase-for-example")));
    assert!(
        got_b[0].contains(&header("passphrase-for-example")),
        "{}",
        got_b[0]
    );
    assert!(!got_b[0].contains(&header("passphrase-for-acme")));
}

#[tokio::test]
async fn one_failing_organisation_does_not_block_the_other() {
    let (web_a, served_a) = zirc_test_ergo::web::serve(vec![(500, "down".into())]);
    let (web_b, served_b) = zirc_test_ergo::web::serve(vec![(401, String::new())]);
    let a = signin_at("alice", "acme.irc.zirc.dev", &web_a, false);
    let b = signin_at("ali", "zirc.irc.zirc.dev", &web_b, false);
    let home = home();
    let account = account_with(&home, several(), &[a, b]);
    let call_a = account
        .get("acme")
        .unwrap()
        .call(home.secrets.as_ref())
        .unwrap();
    let call_b = account
        .get("zirc")
        .unwrap()
        .call(home.secrets.as_ref())
        .unwrap();
    let (first, second) = tokio::join!(
        call_a.client.person(&call_a.credential),
        call_b.client.person(&call_b.credential)
    );
    assert!(first.is_err());
    assert!(second.unwrap().is_none());
    served_a.join().unwrap();
    served_b.join().unwrap();
}

#[test]
fn a_credential_is_only_read_with_its_own_host() {
    // The same handle at two hosts: each reads its own key, and a missing one isn't borrowed.
    let home = home();
    let a = signin_at("alice", "acme.irc.zirc.dev", "https://zirc.dev", false);
    let b = signin_at(
        "alice",
        "chat.example.org",
        "https://chat.example.org",
        false,
    );
    home.secrets.set(&a.key(), "only-for-acme").unwrap();
    let account = Account::open(home.secrets.clone(), several(), [session(a), session(b)]).unwrap();
    assert_eq!(
        account
            .get("acme")
            .unwrap()
            .call(home.secrets.as_ref())
            .unwrap()
            .credential
            .password,
        "only-for-acme"
    );
    assert!(matches!(
        account
            .get("chat.example.org")
            .unwrap()
            .call(home.secrets.as_ref()),
        Err(signin::Error::PassphraseGone { .. })
    ));
}
