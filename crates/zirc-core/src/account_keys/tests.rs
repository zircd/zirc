use super::*;
use crate::account::{Account, OrgState, Policy};
use crate::device_key::Software;
use crate::secrets::File;
use zirc_chat::chat::{Chat, Options};
use zirc_proto::login::Server;

struct Env {
    _dir: tempfile::TempDir,
    secrets: Arc<File>,
    key: Arc<dyn DeviceKey>,
}

fn env(web: &str) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(File(dir.path().join("secrets.toml")));
    secrets.set(&account_id_key(web), "acc-1").unwrap();
    let key: Arc<dyn DeviceKey> = Arc::new(Software::in_file(dir.path().join("device-key")));
    Env {
        _dir: dir,
        secrets,
        key,
    }
}

fn signin(handle: &str, host: &str, web: &str) -> SignIn {
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
        device_key: true,
        media: None,
    }
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

fn account(env: &Env, policy: Policy, signins: Vec<SignIn>) -> Account {
    for s in &signins {
        env.secrets
            .set(&s.key(), &format!("{}-pass", s.handle))
            .unwrap();
    }
    Account::open(
        env.secrets.clone(),
        policy,
        signins.into_iter().map(session),
    )
    .unwrap()
}

fn on() -> Policy {
    Policy::default().with_several(true).with_account_keys(true)
}

const EMPTY: &str = r#"{"organisations":[],"invites":[]}"#;

fn listing(orgs: &[(&str, bool)]) -> String {
    let orgs: Vec<_> = orgs
        .iter()
        .map(|(slug, ready)| {
            format!(
                r#"{{"slug":"{slug}","name":"{slug}","host":"{slug}.irc.zirc.dev","handle":"ali","role":"member","ready":{ready}}}"#
            )
        })
        .collect();
    format!(
        r#"{{"organisations":[{}],"invites":[{{"slug":"inv","name":"Inv"}}]}}"#,
        orgs.join(",")
    )
}

fn zirc_credentials() -> String {
    r#"{"name":"zirc","server":{"host":"zirc.irc.zirc.dev"},"account":{"name":"ali","password":"zirc-fresh"},"nick":"ali"}"#.into()
}

/// The token a request carried.
fn token_of(request: &str) -> zirc_proto::device_key::Parsed {
    let line = request
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("authorization: device "))
        .unwrap_or_else(|| panic!("no device token:\n{request}"));
    zirc_proto::device_key::parse(line.rsplit(' ').next().unwrap()).unwrap()
}

fn body_of(request: &str) -> &str {
    request.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

fn device(env: &Env, web: &str) -> Device {
    Device::new(env.key.clone(), env.secrets.as_ref(), web).unwrap()
}

#[tokio::test]
async fn asks_with_a_short_token_for_zirc_dev_signed_by_the_device_key() {
    let (web, served) = zirc_test_ergo::web::serve(vec![(200, EMPTY.into())]);
    let env = env(&web);
    let listed = Client::new(&web)
        .unwrap()
        .my_organisations(&device(&env, &web))
        .await
        .unwrap();
    assert!(listed.organisations.is_empty());
    let requests = served.join().unwrap();
    assert!(requests[0].starts_with("GET /api/me/organisations "));
    let token = token_of(&requests[0]);
    assert_eq!(token.kid, env.key.public_key().unwrap());
    assert_eq!(token.claims.sub, "acc-1");
    assert_eq!(token.claims.aud, "127.0.0.1");
    assert_eq!(token.claims.exp - token.claims.iat, 60);
}

#[tokio::test]
async fn refresh_merges_new_memberships_and_ends_removed_ones_leaving_the_others() {
    let (web, served) = zirc_test_ergo::web::serve(vec![
        (200, listing(&[("zirc", true), ("later", false)])),
        (200, zirc_credentials()),
    ]);
    let env = env(&web);
    let own = signin("alice", "chat.example.org", "https://chat.example.org");
    let mut account = account(
        &env,
        on(),
        vec![signin("alice", "acme.irc.zirc.dev", &web), own],
    );
    let sealed = |org: &str| crate::sealed::Keys::name(org);
    env.secrets.set(&sealed("acme"), "kept").unwrap();
    env.secrets
        .set(&sealed("chat.example.org"), "kept")
        .unwrap();
    let refreshed = account
        .refresh_organisations(&env.key, &web, |s| session(s.clone()))
        .await
        .unwrap();
    // Acme's sealing keys go as its access does; the others' stay.
    assert_eq!(env.secrets.get(&sealed("acme")), None);
    assert!(env.secrets.get(&sealed("chat.example.org")).is_some());
    assert!(refreshed.asked);
    assert_eq!(refreshed.added, ["zirc"]);
    assert_eq!(refreshed.ended, ["acme"]);
    assert_eq!(refreshed.left.len(), 1);
    assert_eq!(refreshed.invites.len(), 1);
    assert_eq!(account.get("acme").unwrap().state(), OrgState::AccessEnded);
    // The self-hosted one is none of zirc.dev's business.
    assert_ne!(
        account.get("chat.example.org").unwrap().state(),
        OrgState::AccessEnded
    );
    assert_eq!(account.get("zirc").unwrap().signin().handle, "ali");
    assert_eq!(
        env.secrets.get("ali@zirc.irc.zirc.dev").as_deref(),
        Some("zirc-fresh")
    );
    let requests = served.join().unwrap();
    assert!(requests[1].starts_with("POST /api/me/credentials "));
    assert!(body_of(&requests[1]).contains(r#""org":"zirc""#));
}

#[tokio::test]
async fn with_account_keys_off_nothing_is_sent() {
    let (web, served) = zirc_test_ergo::web::serve(Vec::new());
    let env = env(&web);
    let mut account = account(
        &env,
        Policy::default().with_several(true),
        vec![signin("alice", "acme.irc.zirc.dev", &web)],
    );
    let refreshed = account
        .refresh_organisations(&env.key, &web, |s| session(s.clone()))
        .await
        .unwrap();
    assert!(!refreshed.asked);
    assert_ne!(account.get("acme").unwrap().state(), OrgState::AccessEnded);
    assert!(!account.renew_organisation(&env.key, "acme").await.unwrap());
    assert!(served.join().unwrap().is_empty());
}

#[tokio::test]
async fn zirc_dev_with_the_switch_off_changes_nothing() {
    let (web, served) = zirc_test_ergo::web::serve(vec![(404, String::new())]);
    let env = env(&web);
    let mut account = account(&env, on(), vec![signin("alice", "acme.irc.zirc.dev", &web)]);
    let refreshed = account
        .refresh_organisations(&env.key, &web, |s| session(s.clone()))
        .await
        .unwrap();
    assert!(!refreshed.asked);
    assert_ne!(account.get("acme").unwrap().state(), OrgState::AccessEnded);
    served.join().unwrap();
}

#[tokio::test]
async fn a_refused_device_is_signed_in_again_and_never_retried() {
    for (code, expected) in [
        ("device_unknown", Code::DeviceUnknown),
        ("device_revoked", Code::DeviceRevoked),
        ("token_invalid", Code::TokenInvalid),
        ("wrong_audience", Code::WrongAudience),
    ] {
        let body = format!(r#"{{"error":"no","code":"{code}"}}"#);
        let (web, served) = zirc_test_ergo::web::serve(vec![(401, body)]);
        let env = env(&web);
        let err = Client::new(&web)
            .unwrap()
            .my_organisations(&device(&env, &web))
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::Refused { code } if code == expected),
            "{err:?}"
        );
        assert!(err.needs_sign_in());
        assert_eq!(served.join().unwrap().len(), 1, "{code} is not retried");
    }
}

#[tokio::test]
async fn an_expired_or_replayed_token_gets_one_more_try_with_a_fresh_one() {
    for code in ["token_expired", "token_replayed"] {
        let refused = format!(r#"{{"error":"x","code":"{code}"}}"#);
        let (web, served) = zirc_test_ergo::web::serve(vec![(401, refused), (200, EMPTY.into())]);
        let env = env(&web);
        Client::new(&web)
            .unwrap()
            .my_organisations(&device(&env, &web))
            .await
            .unwrap();
        let requests = served.join().unwrap();
        assert_ne!(
            token_of(&requests[0]).claims.jti,
            token_of(&requests[1]).claims.jti
        );
    }
    // Twice is the end of it.
    let refused = r#"{"error":"x","code":"token_expired"}"#;
    let (web, served) =
        zirc_test_ergo::web::serve(vec![(401, refused.into()), (401, refused.into())]);
    let env = env(&web);
    let err = Client::new(&web)
        .unwrap()
        .my_organisations(&device(&env, &web))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        Error::Refused {
            code: Code::TokenExpired
        }
    ));
    assert_eq!(served.join().unwrap().len(), 2);
}

#[tokio::test]
async fn too_many_calls_ask_for_a_pause_and_do_not_sign_out() {
    let (web, served) = zirc_test_ergo::web::serve(vec![(
        429,
        r#"{"error":"slow down","code":"rate_limited"}"#.into(),
    )]);
    let env = env(&web);
    let err = Client::new(&web)
        .unwrap()
        .my_organisations(&device(&env, &web))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::RateLimited { retry_after: 60 }));
    assert!(!err.needs_sign_in());
    served.join().unwrap();
}

#[tokio::test]
async fn a_renamed_handle_gets_fresh_credentials_without_a_browser() {
    let fresh = r#"{"name":"acme","server":{"host":"acme.irc.zirc.dev"},"account":{"name":"alicia","password":"new-pass"},"nick":"alicia"}"#;
    let (web, served) = zirc_test_ergo::web::serve(vec![(200, fresh.into())]);
    let env = env(&web);
    let mut account = account(&env, on(), vec![signin("alice", "acme.irc.zirc.dev", &web)]);
    account.get_mut("acme").unwrap().badge = 4;
    let store = account.store().clone();
    account.get("acme").unwrap().save(&store).unwrap();
    assert!(account.renew_organisation(&env.key, "acme").await.unwrap());
    let session = account.get("acme").unwrap();
    assert_eq!(session.signin().handle, "alicia");
    assert_eq!(session.badge, 4);
    assert_eq!(
        env.secrets.get("alicia@acme.irc.zirc.dev").as_deref(),
        Some("new-pass")
    );
    assert_eq!(env.secrets.get("alice@acme.irc.zirc.dev"), None);
    let requests = served.join().unwrap();
    assert!(body_of(&requests[0]).contains(r#""org":"acme""#));
}

#[tokio::test]
async fn signing_out_of_one_tells_zirc_dev_and_a_failure_does_not_keep_the_person_in() {
    let (web, served) = zirc_test_ergo::web::serve(vec![(200, "{}".into())]);
    let env = env(&web);
    let mut account = account(
        &env,
        on(),
        vec![
            signin("alice", "acme.irc.zirc.dev", &web),
            signin("ali", "zirc.irc.zirc.dev", &web),
        ],
    );
    let (removal, revoked) = account
        .remove_organisation_online(&env.key, "acme", None)
        .await
        .unwrap()
        .unwrap();
    assert!(revoked.failed.is_empty() && !removal.last);
    let requests = served.join().unwrap();
    assert!(requests[0].starts_with("DELETE /api/me/credentials "));
    assert_eq!(token_of(&requests[0]).claims.sub, "acc-1");
    assert!(body_of(&requests[0]).contains(r#""org":"acme""#));
    // zirc.dev being down is no reason to stay signed in to Zirc: the last organisation's key
    // and the device's sign-out from zirc.dev both fail, and it's signed out regardless.
    let (web, served) =
        zirc_test_ergo::web::serve(vec![(500, "down".into()), (500, "down".into())]);
    let env = self::env(&web);
    let mut account = self::account(&env, on(), vec![signin("ali", "zirc.irc.zirc.dev", &web)]);
    let (_, revoked) = account
        .remove_organisation_online(&env.key, "zirc", None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(revoked.failed.len(), 2);
    assert!(account.is_empty());
    assert_eq!(env.secrets.get("ali@zirc.irc.zirc.dev"), None);
    served.join().unwrap();
}

#[tokio::test]
async fn without_account_keys_the_key_registration_is_removed_with_the_organisations_passphrase() {
    let (web, served) = zirc_test_ergo::web::serve(vec![(200, "{}".into())]);
    let env = env(&web);
    let mut account = account(
        &env,
        Policy::default(),
        vec![signin("alice", "acme.irc.zirc.dev", &web)],
    );
    let (_, revoked) = account
        .remove_organisation_online(&env.key, "acme", None)
        .await
        .unwrap()
        .unwrap();
    assert!(revoked.failed.is_empty());
    let request = served.join().unwrap().remove(0);
    let lower = request.to_ascii_lowercase();
    assert!(lower.starts_with("delete /api/me/credentials "));
    assert!(lower.contains("authorization: basic "), "{request}");
    assert!(!lower.contains("authorization: device"));
    assert!(body_of(&request).contains(&env.key.public_key().unwrap()));
}

#[tokio::test]
async fn signing_out_of_the_last_ends_zirc_dev_and_its_browser_session_and_of_one_neither() {
    let (web, served) = zirc_test_ergo::web::serve(vec![
        (200, "{}".into()),
        (200, "{}".into()),
        (200, "{}".into()),
        (200, "{}".into()),
    ]);
    let env = env(&web);
    let mut account = account(
        &env,
        on(),
        vec![
            signin("alice", "acme.irc.zirc.dev", &web),
            signin("ali", "zirc.irc.zirc.dev", &web),
        ],
    );
    let person = Credential {
        name: "alice".into(),
        password: "secret".into(),
        device_key: false,
    };
    // One of two: its key alone, and zirc.dev stays.
    assert!(!account.leaving(&env.key, Some("acme")).ends_zirc_dev());
    account
        .remove_organisation_online(&env.key, "acme", Some((&person, "hash")))
        .await
        .unwrap();
    // The last: its key, the device's sign-out from zirc.dev, and the browser session.
    assert!(account.leaving(&env.key, Some("zirc")).ends_zirc_dev());
    let (removal, revoked) = account
        .remove_organisation_online(&env.key, "zirc", Some((&person, "hash")))
        .await
        .unwrap()
        .unwrap();
    assert!(revoked.failed.is_empty() && removal.last && account.is_empty());
    let requests = served.join().unwrap();
    assert!(requests[0].starts_with("DELETE /api/me/credentials "));
    assert!(requests[1].starts_with("DELETE /api/me/credentials "));
    assert!(requests[2].starts_with("DELETE /api/me/devices/current "));
    assert!(requests[3].starts_with("DELETE /api/session "));
    assert!(body_of(&requests[3]).contains("hash"));
}

#[tokio::test]
async fn signing_out_of_everything_also_signs_the_device_out_of_zirc_dev() {
    let (web, served) = zirc_test_ergo::web::serve(vec![
        (200, "{}".into()),
        (200, "{}".into()),
        (200, "{}".into()),
    ]);
    let env = env(&web);
    let mut account = account(
        &env,
        on(),
        vec![
            signin("alice", "acme.irc.zirc.dev", &web),
            signin("ali", "zirc.irc.zirc.dev", &web),
        ],
    );
    let (out, revoked) = account
        .sign_out_everything_online(&env.key, None)
        .await
        .unwrap();
    assert!(revoked.failed.is_empty() && out.ends_zirc_dev());
    assert!(account.is_empty());
    assert_eq!(env.secrets.get(&account_id_key(&web)), None);
    let requests = served.join().unwrap();
    assert!(requests[0].starts_with("DELETE /api/me/credentials "));
    assert!(requests[1].starts_with("DELETE /api/me/credentials "));
    assert!(requests[2].starts_with("DELETE /api/me/devices/current "));
}

#[tokio::test]
async fn a_phone_fetches_outside_its_lock_and_applies_under_it() {
    let (web, served) = zirc_test_ergo::web::serve(vec![
        (200, listing(&[("acme", true), ("zirc", true)])),
        (200, zirc_credentials()),
        (
            200,
            r#"{"name":"acme","server":{"host":"acme.irc.zirc.dev"},"account":{"name":"alicia","password":"new-pass"},"nick":"alicia"}"#
                .into(),
        ),
    ]);
    let env = env(&web);
    let mut account = account(&env, on(), vec![signin("alice", "acme.irc.zirc.dev", &web)]);
    // The fetch needs no account: only the key, the secrets and what's already known.
    let known = account.connected_hosts();
    let secrets = env.secrets.clone();
    let fetched = fetch_listing(&env.key, secrets.as_ref(), &web, &known)
        .await
        .unwrap()
        .unwrap();
    let refreshed = account.apply_listing(&fetched, |s| session(s.clone()));
    assert_eq!(refreshed.added, ["zirc"]);
    assert!(refreshed.ended.is_empty());
    assert!(account.get("zirc").is_some());
    // The same for credentials after a rename.
    let renewed = fetch_credentials(&env.key, secrets.as_ref(), &web, "acme")
        .await
        .unwrap()
        .unwrap();
    assert!(account.apply_credentials("acme", &renewed).unwrap());
    assert_eq!(account.get("acme").unwrap().signin().handle, "alicia");
    assert!(!account.apply_credentials("gone", &renewed).unwrap());
    assert_eq!(served.join().unwrap().len(), 3);
    // Without an account credential nothing is asked.
    let bare = tempfile::tempdir().unwrap();
    let none = File(bare.path().join("s.toml"));
    assert!(
        fetch_listing(&env.key, &none, &web, &[])
            .await
            .unwrap()
            .is_none()
    );
}
