//! A self-hosted server's files, against the pinned Ergo: accounts zirc.dev made share files
//! through the server's HTTPS side as they do through zirc.dev's, signed in with their names
//! and passwords, which Ergo checks; nobody else does.

mod common;

use serde_json::{Value, json};
use zirc_proto::File;
use zirc_proto::join::SIDECAR_PATH;
use zirc_proto::media::{UploadStarted, done_path, file_path, preview_path};
use zirc_server::secrets::Secrets;

/// A GIF's start: enough for the server to read it's 320 by 240.
const GIF: &[u8] = b"GIF89a\x40\x01\xf0\x00\x00\x00\x00;";

struct Person {
    name: &'static str,
    password: &'static str,
}

const ALICE: Person = Person {
    name: "alice",
    password: "correct-horse-battery-staple",
};
const BOB: Person = Person {
    name: "bob",
    password: "tr0ub4dor-and-three",
};

/// Makes `person`'s account, as zirc.dev does.
async fn make(server: &common::Running, person: &Person) {
    let token = Secrets::read(&server.options.data).unwrap().accounts_token;
    let made = reqwest::Client::new()
        .put(format!(
            "{}{SIDECAR_PATH}/v1/accounts/{}",
            server.https, person.name
        ))
        .bearer_auth(token)
        .json(&json!({"passphrase": person.password}))
        .send()
        .await
        .unwrap();
    assert!(made.status().is_success(), "{}", made.status());
}

/// What the server answers at `path`, the server's own address in `path` taken as its HTTPS
/// side here, as `person`.
fn at(
    server: &common::Running,
    method: reqwest::Method,
    path: &str,
    person: &Person,
) -> reqwest::RequestBuilder {
    let path = path.replace("https://irc.zirc.test", "");
    reqwest::Client::new()
        .request(method, format!("{}{path}", server.https))
        .basic_auth(person.name, Some(person.password))
}

/// Uploads `bytes` named `name` as `person`, and answers the file to share.
async fn upload(
    server: &common::Running,
    person: &Person,
    name: &str,
    mime: &str,
    bytes: &[u8],
) -> File {
    uploaded(server, person, name, mime, bytes).await.0
}

/// `bytes` put with the link `started` gave, with the headers it said, as an app does.
async fn put(
    server: &common::Running,
    started: &UploadStarted,
    bytes: &[u8],
) -> reqwest::StatusCode {
    let mut put =
        reqwest::Client::new().put(started.url.replace("https://irc.zirc.test", &server.https));
    for (name, value) in &started.headers {
        put = put.header(name, value);
    }
    put.body(bytes.to_vec()).send().await.unwrap().status()
}

/// As [`upload`], and the upload it started.
async fn uploaded(
    server: &common::Running,
    person: &Person,
    name: &str,
    mime: &str,
    bytes: &[u8],
) -> (File, UploadStarted) {
    let request = json!({"purpose": "share", "name": name, "mime": mime, "size": bytes.len()});
    let started = at(server, reqwest::Method::POST, "/api/media", person)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(started.status(), 200, "{}", started.text().await.unwrap());
    let started: UploadStarted = started.json().await.unwrap();
    assert_eq!(put(server, &started, bytes).await, 200);
    let done = at(
        server,
        reqwest::Method::POST,
        &done_path(&started.id),
        person,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(done.status(), 200, "{}", done.text().await.unwrap());
    (done.json().await.unwrap(), started)
}

/// The bytes `/m/<id>` sends `person` to, at the link it answers; or its answer, when it
/// doesn't.
async fn fetch(server: &common::Running, url: &str, person: &Person) -> reqwest::Response {
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let path = url.replace("https://irc.zirc.test", "");
    let answer = http
        .get(format!("{}{path}", server.https))
        .basic_auth(person.name, Some(person.password))
        .send()
        .await
        .unwrap();
    if answer.status() != 302 {
        return answer;
    }
    let link = answer.headers()["location"].to_str().unwrap();
    assert!(link.starts_with("https://irc.zirc.test/s/"), "{link}");
    http.get(link.replace("https://irc.zirc.test", &server.https))
        .send()
        .await
        .unwrap()
}

/// `method` at `path` as `person`, sent.
async fn send(
    server: &common::Running,
    method: reqwest::Method,
    path: &str,
    person: &Person,
) -> reqwest::Response {
    at(server, method, path, person).send().await.unwrap()
}

/// A server with alice's and bob's accounts on it; `None` outside Bazel.
async fn with_people() -> Option<common::Running> {
    let server = common::start().await?;
    make(&server, &ALICE).await;
    make(&server, &BOB).await;
    Some(server)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_image_is_what_its_bytes_say_and_anyone_signed_in_gets_it() {
    let Some(server) = with_people().await else {
        return;
    };
    let image = upload(&server, &ALICE, "cat.png", "image/png", GIF).await;
    assert!(
        image.url.starts_with("https://irc.zirc.test/m/"),
        "{}",
        image.url
    );
    // What its bytes say, not what the app did.
    let said = (image.mime.as_str(), image.width, image.height);
    assert_eq!(said, ("image/gif", Some(320), Some(240)));
    let got = fetch(&server, &image.url, &BOB).await;
    assert_eq!(got.status(), 200);
    assert_eq!(got.headers()["content-type"], "image/gif");
    assert_eq!(got.bytes().await.unwrap().as_ref(), GIF);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_checked_once_done_cannot_be_put_again() {
    let Some(server) = with_people().await else {
        return;
    };
    let (image, started) = uploaded(&server, &ALICE, "cat.png", "image/png", GIF).await;
    // The link still has minutes to run: it puts nothing over what was checked.
    let other = b"<svg onload=alert(1)>      ";
    assert_eq!(put(&server, &started, &other[..GIF.len()]).await, 412);
    let got = fetch(&server, &image.url, &BOB).await;
    assert_eq!(got.bytes().await.unwrap().as_ref(), GIF);
}

#[tokio::test(flavor = "multi_thread")]
async fn text_is_previewed_with_its_markup_escaped() {
    let Some(server) = with_people().await else {
        return;
    };
    let notes = upload(
        &server,
        &ALICE,
        "notes.md",
        "text/markdown",
        b"# Plan\n<b>bold</b>",
    )
    .await;
    assert_eq!(notes.mime, "text/markdown");
    let id = notes.url.rsplit('/').next().unwrap();
    let preview = send(&server, reqwest::Method::GET, &preview_path(id), &BOB).await;
    let preview: Value = preview.json().await.unwrap();
    assert_eq!(preview["kind"], "markdown");
    assert_eq!(preview["text"], "# Plan\n<b>bold</b>");
    let html = preview["html"].as_str().unwrap();
    assert!(html.contains("&lt;b&gt;"), "{html}");
}

#[tokio::test(flavor = "multi_thread")]
async fn only_its_uploader_or_an_admin_removes_a_file() {
    let Some(server) = with_people().await else {
        return;
    };
    let image = upload(&server, &ALICE, "cat.png", "image/png", GIF).await;
    let removing = file_path(image.url.rsplit('/').next().unwrap());
    // Bob is a member.
    let theirs = send(&server, reqwest::Method::DELETE, &removing, &BOB).await;
    assert_eq!(theirs.status(), 404);
    let mine = send(&server, reqwest::Method::DELETE, &removing, &ALICE).await;
    assert_eq!(mine.status(), 204);
    let gone = fetch(&server, &image.url, &BOB).await;
    assert_eq!(gone.status(), 404);
    // Once zirc.dev's controller says bob is an owner, he removes anyone's, at once.
    let other = upload(&server, &ALICE, "dog.png", "image/png", GIF).await;
    let controller = Secrets::read(&server.options.data)
        .unwrap()
        .controller_token;
    let teams = reqwest::Client::new()
        .put(format!("{}{SIDECAR_PATH}/v1/teams", server.https))
        .bearer_auth(controller)
        .json(&json!({"channels": [], "owners": ["bob"]}));
    assert!(teams.send().await.unwrap().status().is_success());
    let removing = file_path(other.url.rsplit('/').next().unwrap());
    let owners = send(&server, reqwest::Method::DELETE, &removing, &BOB).await;
    assert_eq!(owners.status(), 204);
}

#[tokio::test(flavor = "multi_thread")]
async fn nobody_else_shares_and_pictures_stay_on_zirc_dev() {
    let Some(server) = with_people().await else {
        return;
    };
    let request = json!({"purpose": "share", "name": "a.txt", "mime": "text/plain", "size": 2});
    let wrong = Person {
        name: "alice",
        password: "not-her-passphrase",
    };
    let refused = at(&server, reqwest::Method::POST, "/api/media", &wrong).json(&request);
    assert_eq!(refused.send().await.unwrap().status(), 401);
    let nobody = reqwest::Client::new()
        .post(format!("{}/api/media", server.https))
        .json(&request);
    assert_eq!(nobody.send().await.unwrap().status(), 401);
    let picture = json!({"purpose": "avatar", "name": "me.png", "mime": "image/png", "size": 2});
    let answer = at(&server, reqwest::Method::POST, "/api/media", &ALICE).json(&picture);
    assert_eq!(answer.send().await.unwrap().status(), 400);
}

/// What asking to share a file as `person` answers.
async fn asked(server: &common::Running, person: &Person) -> reqwest::StatusCode {
    let request = json!({"purpose": "share", "name": "a.txt", "mime": "text/plain", "size": 2});
    let asking = at(server, reqwest::Method::POST, "/api/media", person).json(&request);
    asking.send().await.unwrap().status()
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_passwords_run_out_and_the_right_one_waits_too() {
    let Some(server) = with_people().await else {
        return;
    };
    let wrong = Person {
        name: "alice",
        password: "not-her-passphrase",
    };
    for _ in 0..10 {
        assert_eq!(asked(&server, &wrong).await, 401);
    }
    assert_eq!(asked(&server, &wrong).await, 429);
    assert_eq!(asked(&server, &ALICE).await, 429);
    assert!(asked(&server, &BOB).await.is_success());
}

#[tokio::test(flavor = "multi_thread")]
async fn bytes_go_only_to_their_upload_s_address_and_no_more_than_it_said() {
    let Some(server) = with_people().await else {
        return;
    };
    let request = json!({"purpose": "share", "name": "a.txt", "mime": "text/plain", "size": 2});
    let started = at(&server, reqwest::Method::POST, "/api/media", &ALICE).json(&request);
    let started: UploadStarted = started.send().await.unwrap().json().await.unwrap();
    let url = started.url.replace("https://irc.zirc.test", &server.https);
    let http = reqwest::Client::new();
    let long = http.put(&url).body("too long").send().await.unwrap();
    assert_eq!(long.status(), 413);
    let forged = http
        .put(url.replace("&s=", "&s=x"))
        .body("hi")
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), 403);
    assert_eq!(
        http.put(&url).body("hi").send().await.unwrap().status(),
        200
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_organisation_s_settings_come_from_zirc_dev_with_its_token() {
    let Some(server) = with_people().await else {
        return;
    };
    let path = format!(
        "{}{SIDECAR_PATH}{}",
        server.https,
        zirc_proto::media::SETTINGS_PATH
    );
    let settings = json!({"kinds": ["image"]});
    let http = reqwest::Client::new();
    let without = http.put(&path).json(&settings).send().await.unwrap();
    assert_eq!(without.status(), 401);
    let token = Secrets::read(&server.options.data).unwrap().accounts_token;
    let with = http
        .put(&path)
        .bearer_auth(token)
        .json(&settings)
        .send()
        .await
        .unwrap();
    assert_eq!(with.status(), 204);
    let text = json!({"purpose": "share", "name": "a.txt", "mime": "text/plain", "size": 2});
    let refused = at(&server, reqwest::Method::POST, "/api/media", &ALICE).json(&text);
    let refused = refused.send().await.unwrap();
    assert_eq!(refused.status(), 403);
    let said: Value = refused.json().await.unwrap();
    assert_eq!(said["error"], "This organisation doesn't take text files.");
}

/// A stand-in for imgproxy at a free port: it reads the source its path names, beside the
/// server, and answers what it'd resize it to and its bytes.
async fn imgproxy() -> String {
    use axum::extract::Path;
    use axum::routing::get;
    async fn resize(Path(rest): Path<String>) -> (axum::http::StatusCode, String) {
        let (options, encoded) = rest.rsplit_once('/').unwrap();
        let (encoded, format) = encoded.rsplit_once('.').unwrap();
        let source =
            base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, encoded)
                .unwrap();
        let source = String::from_utf8(source).unwrap();
        let got = reqwest::get(&source).await.unwrap();
        let status = got.status().as_u16();
        let bytes = got.bytes().await.unwrap();
        let said = format!("{options} {format} {status} {}", bytes.len());
        (axum::http::StatusCode::OK, said)
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let at = listener.local_addr().unwrap();
    let router = axum::Router::new().route("/insecure/{*rest}", get(resize));
    tokio::spawn(async move { axum::serve(listener, router).await });
    format!("http://{at}")
}

#[tokio::test(flavor = "multi_thread")]
async fn images_are_resized_by_imgproxy_reading_them_beside_the_server() {
    let imgproxy = imgproxy().await;
    let Some(server) = common::start_with(|options| {
        // A port of its own each time the server is started again, as the others are.
        let [beside] = zirc_test_ergo::free_ports();
        options.beside_listen = Some(([127, 0, 0, 1], beside).into());
        options.beside_url = format!("http://127.0.0.1:{beside}");
        options.imgproxy = Some(imgproxy.clone());
    })
    .await
    else {
        return;
    };
    make(&server, &ALICE).await;
    let image = upload(&server, &ALICE, "cat.gif", "image/gif", GIF).await;
    let small = send(
        &server,
        reqwest::Method::GET,
        &format!("{}?w=100", image.url),
        &ALICE,
    )
    .await;
    assert_eq!(small.status(), 200);
    assert_eq!(small.headers()["content-type"], "image/webp");
    // The next width up, and the whole original, read beside the server.
    let said = small.text().await.unwrap();
    assert_eq!(said, format!("rs:fit:256:0 webp 200 {}", GIF.len()));
    let jpeg = send(
        &server,
        reqwest::Method::GET,
        &format!("{}?w=2000&f=jpeg", image.url),
        &ALICE,
    )
    .await;
    assert_eq!(jpeg.headers()["content-type"], "image/jpeg");
    // The original, asked for, is a link to it.
    let original = fetch(&server, &format!("{}?original", image.url), &ALICE).await;
    assert_eq!(original.bytes().await.unwrap().as_ref(), GIF);
}

/// A stand-in for the media worker at a free port: every video is a second long and 640 by
/// 360, and what it makes is the original's size, put where the job says, before it answers.
async fn worker() -> String {
    use axum::Json;
    use axum::routing::post;
    async fn probe() -> Json<Value> {
        Json(json!({
            "duration_ms": 1000, "width": 640, "height": 360,
            "video": "h264", "audio": null, "container": "mov,mp4"
        }))
    }
    async fn jobs(Json(job): Json<zirc_media_worker::Job>) -> axum::http::StatusCode {
        let original = reqwest::get(&job.source)
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let made = format!("{:?} of {} bytes", job.make, original.len());
        let mut put = reqwest::Client::new().put(&job.put.url);
        for (name, value) in &job.put.headers {
            put = put.header(name, value);
        }
        assert_eq!(put.body(made).send().await.unwrap().status(), 200);
        axum::http::StatusCode::ACCEPTED
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let at = listener.local_addr().unwrap();
    let router = axum::Router::new()
        .route("/probe", post(probe))
        .route("/jobs", post(jobs));
    tokio::spawn(async move { axum::serve(listener, router).await });
    format!("http://{at}")
}

#[tokio::test(flavor = "multi_thread")]
async fn videos_play_from_copies_the_worker_makes_when_first_asked_for() {
    let worker = worker().await;
    let Some(server) = common::start_with(|options| {
        // A port of its own each time the server is started again, as the others are.
        let [beside] = zirc_test_ergo::free_ports();
        options.beside_listen = Some(([127, 0, 0, 1], beside).into());
        options.beside_url = format!("http://127.0.0.1:{beside}");
        options.media_worker = Some(worker.clone());
    })
    .await
    else {
        return;
    };
    make(&server, &ALICE).await;
    let token = Secrets::read(&server.options.data).unwrap().accounts_token;
    let settings = reqwest::Client::new()
        .put(format!(
            "{}{SIDECAR_PATH}{}",
            server.https,
            zirc_proto::media::SETTINGS_PATH
        ))
        .bearer_auth(token)
        .json(&json!({"video": true}));
    assert_eq!(settings.send().await.unwrap().status(), 204);
    let video = upload(&server, &ALICE, "talk.mp4", "video/mp4", b"not really").await;
    let read = (video.width, video.height, video.duration_ms);
    assert_eq!(read, (Some(640), Some(360), Some(1000)));
    let play = format!("{}?r=1000", video.url);
    // Made the first time it's asked for, then played from a link.
    let making = fetch(&server, &play, &ALICE).await;
    assert_eq!(making.status(), 202);
    let copy = fetch(&server, &play, &ALICE).await;
    assert_eq!(copy.headers()["content-type"], "video/mp4");
    assert_eq!(
        copy.text().await.unwrap(),
        "Copy { height: 720 } of 10 bytes"
    );
}
