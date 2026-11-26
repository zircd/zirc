use super::*;
use zirc_core::settings::Settings;

fn config(web: &str, orgs: &[&str]) -> String {
    let org = |host: &str| {
        format!(
            "web = \"{web}\"\n[{{p}}server]\nhost = \"{host}.irc.zirc.dev\"\n\
             [{{p}}account]\nname = \"m\"\npassword = \"p\"\ndevice_key = true\n"
        )
    };
    let mut text = org(orgs[0]).replace("{p}", "");
    for host in &orgs[1..] {
        text.push_str("[[org]]\n");
        text.push_str(&org(host).replace("{p}", "org."));
    }
    text
}

fn how(local: bool) -> How {
    How {
        yes: true,
        local,
        no_daemon: true,
    }
}

async fn run(
    text: &str,
    scope: Scope,
    kind: Kind,
    how: &How,
    answer: &str,
) -> (
    std::path::PathBuf,
    tempfile::TempDir,
    Result<String, String>,
) {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(paths::config(home.path()), text).unwrap();
    // A key to forget.
    zirc_core::device_key::DeviceKey::public_key(&Software::machine(home.path()));
    let mut out = Vec::new();
    let result = sign_out(
        home.path(),
        &scope,
        kind,
        how,
        &mut answer.as_bytes(),
        &mut out,
    )
    .await;
    let config = paths::config(home.path());
    (
        config,
        home,
        result
            .map(|()| String::from_utf8(out).unwrap())
            .map_err(|err| err.to_string()),
    )
}

#[tokio::test]
async fn one_of_two_organisations_leaves_the_other_and_zirc_dev() {
    let (web, requests) = zirc_test_ergo::web::serve(vec![(200, "{}".into())]);
    let text = config(&web, &["zirc", "globex"]);
    let (path, home, out) = run(&text, Scope::One("zirc".into()), Kind::Own, &how(false), "").await;
    let out = out.unwrap();
    assert!(out.contains("Sign out of Zirc?"), "{out}");
    assert!(out.contains("Revoked this machine's key for zirc"), "{out}");
    assert!(out.contains("Signed out of Zirc."), "{out}");
    assert!(!out.contains("Signed out of zirc.dev."), "{out}");
    let left = Settings::from_toml(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(left.server.host, "globex.irc.zirc.dev");
    assert!(paths::device_key(home.path()).exists());
    let served = requests.join().unwrap();
    assert!(
        served[0].starts_with("DELETE /api/me/credentials"),
        "{served:?}"
    );
    assert!(served[0].contains(r#""org":"zirc""#), "{served:?}");
}

#[tokio::test]
async fn the_last_organisation_signs_out_of_zirc_dev_and_forgets_the_key() {
    let (web, _requests) = zirc_test_ergo::web::serve(vec![(200, "{}".into())]);
    let (path, home, out) = run(
        &config(&web, &["zirc"]),
        Scope::One("zirc".into()),
        Kind::Own,
        &how(false),
        "",
    )
    .await;
    let out = out.unwrap();
    assert!(out.contains("also signs you out of zirc.dev"), "{out}");
    assert!(out.contains("Signed out of zirc.dev."), "{out}");
    assert!(!path.exists());
    assert!(!paths::device_key(home.path()).exists());
}

#[tokio::test]
async fn offline_it_signs_out_here_and_says_the_key_was_not_revoked() {
    let (path, _home, out) = run(
        &config("http://127.0.0.1:1", &["zirc"]),
        Scope::Everything,
        Kind::Everything,
        &how(false),
        "",
    )
    .await;
    let out = out.unwrap();
    assert!(out.contains("Couldn't reach zirc.dev for zirc"), "{out}");
    assert!(out.contains("zirc.dev still accepts it for zirc"), "{out}");
    assert!(!path.exists());
}

#[tokio::test]
async fn local_touches_no_network() {
    let (path, _home, out) = run(
        &config("http://127.0.0.1:1", &["zirc"]),
        Scope::Everything,
        Kind::Everything,
        &how(true),
        "",
    )
    .await;
    let out = out.unwrap();
    assert!(!out.contains("Couldn't reach"), "{out}");
    assert!(out.contains("still accepts it for zirc"), "{out}");
    assert!(!path.exists());
}

#[tokio::test]
async fn it_asks_and_a_no_changes_nothing() {
    let text = config("http://127.0.0.1:1", &["zirc", "globex"]);
    let ask = How {
        yes: false,
        local: true,
        no_daemon: true,
    };
    let (path, _home, out) = run(&text, Scope::Everything, Kind::Everything, &ask, "n\n").await;
    let out = out.unwrap();
    assert!(
        out.contains("Signs you out of Zirc, Globex and zirc.dev on this device."),
        "{out}"
    );
    assert!(out.contains("Sign out of everything? [y/N]"), "{out}");
    assert!(out.ends_with("Nothing changed.\n"), "{out}");
    assert_eq!(std::fs::read_to_string(path).unwrap(), text);
    let (path, _home, out) = run(&text, Scope::Everything, Kind::Everything, &ask, "y\n").await;
    out.unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn logout_takes_an_organisation_off_by_its_name() {
    // One of the machine's own: taken off, as `zirc orgs remove` does, and the other stays.
    let (path, _home, out) = run(
        &config("http://127.0.0.1:1", &["zirc", "globex"]),
        Scope::One("zirc".into()),
        Kind::Own,
        &how(true),
        "",
    )
    .await;
    let out = out.unwrap();
    assert!(out.contains("Signed out of Zirc."), "{out}");
    assert!(out.contains("You stay signed in to Globex."), "{out}");
    assert!(path.exists());
}

#[tokio::test]
async fn nothing_to_sign_out_of_is_said() {
    let home = tempfile::tempdir().unwrap();
    let mut out = Vec::new();
    sign_out(
        home.path(),
        &Scope::Everything,
        Kind::Everything,
        &how(true),
        &mut "".as_bytes(),
        &mut out,
    )
    .await
    .unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "This machine isn't logged in to anything.\n"
    );
}
