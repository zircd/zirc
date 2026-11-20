//! Sealed parts of posts: sealed with the organisation's key as one machine posts them, and
//! opened by another holding it, while the server and plain IRC clients see only `[sealed]`.

mod common;

use common::{Machine, next_post, two_machines};
use zirc_core::ipc::{Request, Response, Target};
use zirc_core::sealed::{Keys, machine_store, new_key};
use zirc_proto::Event;
use zirc_test_ergo::require_ergo;

/// What the test machines call their organisation: their server's host.
const ORG: &str = "127.0.0.1";

const SECRET: &str = "sk_test_51abc";

fn sealed_post(repo: &std::path::Path, text: &str) -> Request {
    Request::SealedPost {
        repo: repo.to_owned(),
        session: "cc-1".into(),
        text: text.into(),
        to: Target::Agents,
        reply: None,
    }
}

fn hold(machine: &Machine, key: &zirc_core::sealed::Key) {
    Keys::new(&machine_store(machine.home.path()))
        .add(key)
        .unwrap();
}

#[tokio::test]
async fn a_sealed_part_opens_only_on_a_machine_holding_the_key() {
    let ergo = require_ergo!();
    let [(alpha, alpha_clone), (beta, _beta_clone)] = two_machines(&ergo).await;
    let (ada, mut ada_in) = zirc_test_irc::person(&ergo, "ada").await;
    zirc_test_irc::joins(&ada, &mut ada_in, "#agents").await;

    // Without a key, nothing goes out.
    let refused = alpha
        .try_request(sealed_post(alpha_clone.path(), "the key is ||sk_1||"))
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("no key for 127.0.0.1"),
        "{refused}"
    );

    let key = new_key(ORG).unwrap();
    hold(&alpha, &key);
    hold(&beta, &key);
    let mut watching = beta.watch(None).await;
    alpha
        .request(sealed_post(
            alpha_clone.path(),
            &format!("staging: ||{SECRET}|| and `make || true`"),
        ))
        .await;

    // A plain IRC client, as the server keeps it, has only the placeholder and ciphertext.
    let line = zirc_test_irc::next_matching(&mut ada_in, |line| line.command == "PRIVMSG").await;
    assert!(!line.to_string().contains(SECRET), "{line}");
    assert!(line.tag(zirc_proto::sealed::TAG).is_some(), "{line}");

    let delivery = next_post(&mut watching).await;
    assert_eq!(
        delivery.message.event,
        Event::Text {
            body: "staging: [sealed] and `make || true`".into()
        }
    );
    let id = delivery.id.clone().expect("the server's id");
    let unseal = |part| Request::Unseal {
        message: id[..8].to_owned(),
        part,
    };
    assert_eq!(
        beta.request(unseal(1)).await,
        Response::Unsealed {
            value: SECRET.into()
        }
    );
    let past = beta.try_request(unseal(2)).await.unwrap_err();
    assert!(past.to_string().contains("has 1 sealed part"), "{past}");

    // Forgotten, it no longer opens there.
    Keys::new(&machine_store(beta.home.path()))
        .forget(ORG, &key.id())
        .unwrap();
    let forgotten = beta.try_request(unseal(1)).await.unwrap_err();
    assert!(forgotten.to_string().contains("no key"), "{forgotten}");
}
