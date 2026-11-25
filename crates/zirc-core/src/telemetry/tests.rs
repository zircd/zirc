use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use zirc_proto::telemetry::{AppOpened, CommandRun, Feature, FeatureUsed};

use super::*;

fn settings(extra: &str, host: &str) -> Settings {
    Settings::from_toml(&format!(
        "{extra}\nposthog_key = \"phc_test\"\nposthog_host = \"{host}\"\n[server]\nhost = \"acme.irc.zirc.dev\"\n"
    ))
    .unwrap()
}

fn none(_: &str) -> Option<String> {
    None
}

fn command(name: &str) -> Event {
    Event::CommandRun(CommandRun {
        command: name.into(),
        agent: false,
    })
}

/// A fake `PostHog`: answers each request with `status`, and hands over each request's
/// path and JSON body.
async fn posthog(
    status: u16,
) -> (
    String,
    tokio::sync::mpsc::UnboundedReceiver<(String, Value)>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = format!("http://{}", listener.local_addr().unwrap());
    let (requests, received) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            let (head, body_len) = loop {
                let n = stream.read(&mut buf).await.unwrap();
                request.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(end) = text.find("\r\n\r\n") {
                    let len = text[..end]
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    break (end + 4, len);
                }
            };
            while request.len() < head + body_len {
                let n = stream.read(&mut buf).await.unwrap();
                request.extend_from_slice(&buf[..n]);
            }
            let text = String::from_utf8_lossy(&request[..head]).into_owned();
            let path = text.split(' ').nth(1).unwrap().to_owned();
            let body = serde_json::from_slice(&request[head..]).unwrap();
            let _ = requests.send((path, body));
            let reply = format!(
                "HTTP/1.1 {status} OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}"
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
        }
    });
    (host, received)
}

#[test]
fn is_on_unless_the_setting_or_the_environment_turns_it_off() {
    assert!(enabled(None, none));
    assert!(enabled(Some(true), none));
    assert!(!enabled(Some(false), none));
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    };
    assert!(!enabled(None, env(&[("DO_NOT_TRACK", "1")])));
    assert!(!enabled(None, env(&[("DO_NOT_TRACK", "true")])));
    assert!(enabled(None, env(&[("DO_NOT_TRACK", "0")])));
    assert!(enabled(None, env(&[("DO_NOT_TRACK", "")])));
    assert!(!enabled(None, env(&[("ZIRC_TELEMETRY", "0")])));
    assert!(!enabled(Some(true), env(&[("ZIRC_TELEMETRY", "off")])));
    assert!(enabled(None, env(&[("ZIRC_TELEMETRY", "1")])));
    assert!(
        !enabled(None, env(&[("TEST_TMPDIR", "/tmp/bazel")])),
        "tests aren't usage"
    );
    assert!(!enabled(None, env(&[("CI", "true")])), "nor is CI");
    assert!(enabled(None, env(&[("CI", "false")])));
    // Turning it on in the environment doesn't overrule config.toml turning it off.
    assert!(!enabled(Some(false), env(&[("ZIRC_TELEMETRY", "1")])));
}

#[test]
fn does_nothing_when_turned_off_or_with_no_key() {
    let home = tempfile::tempdir().unwrap();
    let no_key =
        Settings::from_toml("posthog_key = \"\"\n[server]\nhost = \"acme.irc.zirc.dev\"\n")
            .unwrap();
    let off = settings("telemetry = false", DEFAULT_HOST);
    let not_tracked = |name: &str| (name == "DO_NOT_TRACK").then(|| "1".to_owned());
    // A config.toml that can't be read may be one that turns it off.
    std::fs::write(crate::paths::config(home.path()), "telemetry = nope").unwrap();
    for telemetry in [
        Telemetry::with(home.path(), Client::Cli, Some(&no_key), none),
        Telemetry::with(home.path(), Client::Cli, Some(&off), none),
        Telemetry::with(home.path(), Client::Cli, None, not_tracked),
        Telemetry::load(home.path(), Client::Cli),
    ] {
        assert!(!telemetry.is_on());
        telemetry.record(&command("who"));
        telemetry.notice(true, &mut Vec::new());
    }
    assert!(!dir(home.path()).exists(), "nothing is written");
}

#[test]
fn sends_to_zircs_project_unless_told_otherwise() {
    let home = tempfile::tempdir().unwrap();
    let bare = Settings::from_toml("[server]\nhost = \"acme.irc.zirc.dev\"\n").unwrap();
    for settings in [None, Some(&bare)] {
        let telemetry = Telemetry::with(home.path(), Client::Cli, settings, none);
        let on = telemetry.0.as_ref().expect("on by default");
        assert_eq!(
            (on.key.as_str(), on.host.as_str()),
            (DEFAULT_KEY, DEFAULT_HOST)
        );
    }
    let own = Telemetry::with(
        home.path(),
        Client::Cli,
        Some(&settings("", "https://ph.test/")),
        none,
    );
    let on = own.0.as_ref().unwrap();
    assert_eq!(
        (on.key.as_str(), on.host.as_str()),
        ("phc_test", "https://ph.test")
    );
}

#[tokio::test]
async fn sends_the_queue_as_one_batch_with_the_apps_context() {
    let home = tempfile::tempdir().unwrap();
    let (host, mut requests) = posthog(200).await;
    let telemetry = Telemetry::with(home.path(), Client::Tui, Some(&settings("", &host)), none);
    telemetry.record(&Event::AppOpened(AppOpened {}));
    telemetry.record(&Event::FeatureUsed(FeatureUsed {
        feature: Feature::Reaction,
    }));
    let recorded = queued(home.path());
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[1]["properties"]["feature"], "reaction");
    assert_eq!(recorded[1]["properties"]["client"], "tui");
    assert_eq!(
        recorded[1]["properties"]["version"],
        zirc_proto::version::get()
    );

    telemetry.send(&http().unwrap()).await;
    let (path, body) = requests.recv().await.unwrap();
    assert_eq!(path, "/batch/");
    assert_eq!(body["api_key"], "phc_test");
    let batch = body["batch"].as_array().unwrap();
    let events: Vec<_> = batch.iter().map(|event| event["event"].clone()).collect();
    assert_eq!(events, ["app_opened", "feature_used"]);
    let install = std::fs::read_to_string(dir(home.path()).join("install-id")).unwrap();
    assert_eq!(install.len(), 36, "a UUID: {install}");
    assert!(
        batch
            .iter()
            .all(|event| event["distinct_id"] == install.as_str())
    );
    assert!(batch[0]["timestamp"].is_string());
    assert!(
        queued(home.path()).is_empty(),
        "what's sent leaves the queue"
    );

    // Nothing queued, nothing sent; the install keeps its id.
    telemetry.send(&http().unwrap()).await;
    telemetry.record(&command("who"));
    telemetry.send(&http().unwrap()).await;
    let (_, body) = requests.recv().await.unwrap();
    assert_eq!(body["batch"].as_array().unwrap().len(), 1);
    assert_eq!(body["batch"][0]["distinct_id"], install.as_str());
}

#[tokio::test]
async fn links_the_install_to_the_account_once_signed_in() {
    let home = tempfile::tempdir().unwrap();
    let (host, mut requests) = posthog(200).await;
    let telemetry = Telemetry::with(home.path(), Client::Cli, Some(&settings("", &host)), none);
    telemetry.record(&command("status"));
    telemetry.send(&http().unwrap()).await;
    let (_, before) = requests.recv().await.unwrap();
    let install = before["batch"][0]["distinct_id"]
        .as_str()
        .unwrap()
        .to_owned();

    link(home.path(), "acct_7f3a");
    telemetry.record(&command("watch"));
    telemetry.send(&http().unwrap()).await;
    let (_, body) = requests.recv().await.unwrap();
    let batch = body["batch"].as_array().unwrap();
    assert_eq!(batch[0]["event"], "$identify");
    assert_eq!(batch[0]["distinct_id"], "acct_7f3a");
    assert_eq!(
        batch[0]["properties"]["$anon_distinct_id"],
        install.as_str()
    );
    assert_eq!(batch[1]["event"], "command_run");
    assert_eq!(batch[1]["distinct_id"], "acct_7f3a");

    // Linked once.
    telemetry.record(&command("who"));
    telemetry.send(&http().unwrap()).await;
    let (_, body) = requests.recv().await.unwrap();
    let events: Vec<_> = body["batch"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["event"].clone())
        .collect();
    assert_eq!(events, ["command_run"]);
}

#[tokio::test]
async fn drops_what_cant_be_sent() {
    let home = tempfile::tempdir().unwrap();
    let (host, mut requests) = posthog(503).await;
    let telemetry = Telemetry::with(home.path(), Client::Cli, Some(&settings("", &host)), none);
    link(home.path(), "acct_7f3a");
    telemetry.record(&command("post"));
    telemetry.send(&http().unwrap()).await;
    assert!(requests.recv().await.is_some());
    assert!(queued(home.path()).is_empty());
    assert!(
        !dir(home.path()).join("identified").exists(),
        "linking is tried again"
    );

    // Nobody listening at all.
    let gone = Telemetry::with(
        home.path(),
        Client::Cli,
        Some(&settings("", "http://127.0.0.1:9")),
        none,
    );
    gone.record(&command("post"));
    gone.send(&http().unwrap()).await;
    assert!(queued(home.path()).is_empty());
}

#[test]
fn tells_once_and_only_at_a_terminal() {
    let home = tempfile::tempdir().unwrap();
    let telemetry = Telemetry::with(
        home.path(),
        Client::Cli,
        Some(&settings("", DEFAULT_HOST)),
        none,
    );
    let mut out = Vec::new();
    telemetry.notice(false, &mut out);
    assert!(out.is_empty(), "not to a pipe");
    telemetry.notice(true, &mut out);
    assert_eq!(String::from_utf8(out).unwrap(), format!("{NOTICE}\n"));
    let mut again = Vec::new();
    telemetry.notice(true, &mut again);
    assert!(again.is_empty(), "once");
    assert!(NOTICE.contains("ZIRC_TELEMETRY=0") && NOTICE.contains("telemetry = false"));
}

#[test]
fn stops_queueing_when_full() {
    let home = tempfile::tempdir().unwrap();
    let telemetry = Telemetry::with(
        home.path(),
        Client::Cli,
        Some(&settings("", DEFAULT_HOST)),
        none,
    );
    let queue = dir(home.path()).join("queue.jsonl");
    std::fs::create_dir_all(dir(home.path())).unwrap();
    let full = usize::try_from(QUEUE_MAX_BYTES).unwrap();
    std::fs::write(&queue, "x".repeat(full)).unwrap();
    telemetry.record(&command("who"));
    assert_eq!(std::fs::metadata(&queue).unwrap().len(), QUEUE_MAX_BYTES);
}

#[test]
fn an_app_with_its_own_setting_and_version() {
    let home = tempfile::tempdir().unwrap();
    let off = Telemetry::for_app(home.path(), Client::Desktop, "0.3.3", Some(false));
    assert!(!off.is_on());
    std::fs::write(
        crate::paths::config(home.path()),
        "telemetry = false\n[server]\nhost = \"x\"\n",
    )
    .unwrap();
    let on = Telemetry::for_app(home.path(), Client::Desktop, "0.3.3", Some(true));
    assert_eq!(on.is_on(), enabled(None, |name| std::env::var(name).ok()));
    let unset = Telemetry::for_app(home.path(), Client::Desktop, "0.3.3", None);
    assert!(
        !unset.is_on(),
        "config.toml's setting when the app has none"
    );
}

#[test]
fn says_when_the_app_was_updated_since_it_last_opened() {
    let home = tempfile::tempdir().unwrap();
    let at =
        |version: &str| Telemetry::build(home.path(), Client::Desktop, version, None, None, none);
    at("0.3.2").opened();
    at("0.3.2").opened();
    at("0.3.3").opened();
    let names: Vec<_> = queued(home.path())
        .iter()
        .map(|event| event["event"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        names,
        ["app_opened", "app_opened", "app_opened", "update_installed"]
    );
    let installed = &queued(home.path())[3]["properties"];
    assert_eq!(
        (installed["from"].as_str(), installed["to"].as_str()),
        (Some("0.3.2"), Some("0.3.3"))
    );
    assert_eq!(installed["version"], "0.3.3");
}

#[test]
fn counts_direct_messages_by_whom_to() {
    assert_eq!(said_to("#zirc"), None);
    assert_eq!(said_to("bob"), Some(Feature::Dm));
    assert_eq!(said_to("mattias-mbp/cc-1"), Some(Feature::AgentDm));
    assert_eq!(
        said_to(&format!("{}bob-carol", crate::chat::GROUP_PREFIX)),
        Some(Feature::Dm)
    );
}

#[tokio::test]
async fn stops_sending_once_config_toml_turns_it_off() {
    let home = tempfile::tempdir().unwrap();
    let (host, mut requests) = posthog(200).await;
    let telemetry = Telemetry::with(home.path(), Client::Zircd, Some(&settings("", &host)), none);
    telemetry.record(&command("watch"));
    std::fs::write(
        crate::paths::config(home.path()),
        "telemetry = false\n[server]\nhost = \"acme.irc.zirc.dev\"\n",
    )
    .unwrap();
    telemetry.send(&http().unwrap()).await;
    assert!(queued(home.path()).is_empty(), "what was queued is dropped");
    assert!(requests.try_recv().is_err(), "and not sent");
}

#[test]
fn an_install_marked_internal_stays_so() {
    let home = tempfile::tempdir().unwrap();
    let marked = |name: &str| (name == "ZIRC_TELEMETRY_INTERNAL").then(|| "1".to_owned());
    let unmarked = |name: &str| (name == "ZIRC_TELEMETRY_INTERNAL").then(|| "0".to_owned());
    let bare = Settings::from_toml("[server]\nhost = \"acme.irc.zirc.dev\"\n").unwrap();
    let context = |env: &dyn Fn(&str) -> Option<String>| {
        let telemetry = Telemetry::with(home.path(), Client::Zircd, Some(&bare), env);
        telemetry.0.as_ref().unwrap().context.internal
    };
    assert!(!context(&none), "not unless it's said");
    assert!(!context(&unmarked));
    assert!(context(&marked));
    // As zircd started by the system later, without the variable.
    assert!(context(&none), "remembered");
    let telemetry = Telemetry::with(home.path(), Client::Cli, Some(&bare), none);
    telemetry.record(&command("who"));
    assert_eq!(queued(home.path())[0]["properties"]["internal"], true);
}

#[test]
fn an_organisations_events_are_its_group() {
    let home = tempfile::tempdir().unwrap();
    let bare = Settings::from_toml("[server]\nhost = \"acme.irc.zirc.dev\"\n").unwrap();
    let telemetry = Telemetry::with(home.path(), Client::Zircd, Some(&bare), none);
    link_organisation(home.path(), "acme", "0c19b348-e62d-48ec-961b-b3426a7c532d");
    // A name zirc couldn't keep is never a path.
    link_organisation(home.path(), "../acme", "elsewhere");
    let intent = Event::IntentPosted(zirc_proto::telemetry::IntentPosted {});
    telemetry.record_in(&intent, Some("acme"));
    telemetry.record_in(&intent, Some("globex"));
    telemetry.record_in(&intent, Some("../acme"));
    telemetry.record(&intent);
    let recorded = queued(home.path());
    assert_eq!(
        recorded[0]["properties"]["$groups"],
        serde_json::json!({"organisation": "0c19b348-e62d-48ec-961b-b3426a7c532d"})
    );
    for unlinked in &recorded[1..] {
        assert!(
            unlinked["properties"].get("$groups").is_none(),
            "{unlinked:?}"
        );
    }
    assert!(!home.path().join("telemetry/acme").exists());
}

#[test]
fn a_sign_in_links_the_account_and_each_organisation() {
    let home = tempfile::tempdir().unwrap();
    let credentials: zirc_proto::login::Credentials = serde_json::from_value(serde_json::json!({
        "server": {"host": "acme.irc.zirc.dev"},
        "account_id": "acct_7f3a",
        "organisation_id": "org-acme",
        "orgs": [
            {"name": "acme", "server": {"host": "acme.irc.zirc.dev"}, "organisation_id": "org-acme"},
            {"name": "globex", "server": {"host": "globex.irc.zirc.dev"}, "organisation_id": "org-globex"},
            {"name": "initech", "server": {"host": "initech.irc.zirc.dev"}},
        ],
    }))
    .unwrap();
    link_login(home.path(), &credentials, "https://zirc.dev");
    let dir = dir(home.path());
    assert_eq!(read(&dir.join("account")).as_deref(), Some("acct_7f3a"));
    assert_eq!(organisation(&dir, "acme").as_deref(), Some("org-acme"));
    assert_eq!(organisation(&dir, "globex").as_deref(), Some("org-globex"));
    assert_eq!(
        organisation(&dir, "initech"),
        None,
        "no id, nothing to group by"
    );
}
