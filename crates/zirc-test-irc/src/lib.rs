//! People on a test Ergo, for zirc's integration tests: plain IRC clients that talk to
//! what's under test, register accounts as zirc.dev does, and wait for what the server
//! says. zirc-irc's own tests can't use it, since it's built on zirc-irc.

pub mod faults;

use std::future::Future;
use std::time::Duration;

use zirc_irc::{Client, Config, Incoming, Line};
use zirc_test_ergo::Ergo;
use zirc_test_ergo::wait::Probe;

/// How long anything waited for may take.
pub const WAIT: Duration = Duration::from_secs(10);

/// How long [`eventually`] waits.
pub const EVENTUALLY: Duration = Duration::from_secs(15);

/// Calls `probe` until it's done (`true`, or `Some(value)`, which it returns), failing the
/// test with "no {what} in 15s" otherwise: the async twin of `zirc_test_ergo::wait`. It looks
/// every 50 ms, so a test never sleeps a fixed time and hopes.
///
/// # Panics
///
/// When it takes longer than [`EVENTUALLY`].
pub async fn eventually<P: Probe, F: Future<Output = P>>(
    what: &str,
    mut probe: impl FnMut() -> F,
) -> P::Found {
    tokio::time::timeout(EVENTUALLY, async {
        loop {
            if let Some(found) = probe().await.found() {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {what} in {EVENTUALLY:?}"))
}

/// A client config for `ergo` as `nick`, without TLS, as the test server listens.
#[must_use]
pub fn config(ergo: &Ergo, nick: &str) -> Config {
    Config {
        tls: false,
        ..Config::new("127.0.0.1", ergo.port(), nick)
    }
}

/// A person's client on `ergo`, connected as `nick`.
///
/// # Panics
///
/// When it can't connect.
pub async fn person(ergo: &Ergo, nick: &str) -> (Client, Incoming) {
    zirc_irc::connect(&config(ergo, nick))
        .await
        .expect("connected to Ergo")
}

/// Joins `client` to `channel` and waits until the server has it there, as its end of the
/// channel's names says: from then on it hears what's said there. A fixed sleep instead
/// is slow on a quiet machine and too short on a busy one.
///
/// # Panics
///
/// When the server doesn't say so within [`WAIT`].
pub async fn joins(client: &Client, incoming: &mut Incoming, channel: &str) {
    client.join(channel).unwrap();
    // RPL_ENDOFNAMES, which follows the JOIN once the server has the client in the channel.
    next_matching(incoming, |line| {
        line.command == "366"
            && line
                .params
                .get(1)
                .is_some_and(|it| it.eq_ignore_ascii_case(channel))
    })
    .await;
}

/// The next line from the server that `matches`, skipping the rest.
///
/// # Panics
///
/// When the connection ends or none comes within [`WAIT`].
pub async fn next_matching(incoming: &mut Incoming, matches: impl Fn(&Line) -> bool) -> Line {
    tokio::time::timeout(WAIT, async {
        loop {
            let line = incoming.recv().await.expect("still connected");
            if matches(&line) {
                return line;
            }
        }
    })
    .await
    .expect("no matching line in time")
}

/// The next whole message from the server that `matches`, skipping the rest: a multiline
/// batch joined into one `PRIVMSG` with its opening line's tags (its `msgid` among them),
/// as apps see it, through `batches`, kept between calls.
///
/// # Panics
///
/// As [`next_matching`] does.
pub async fn next_whole_matching(
    incoming: &mut Incoming,
    batches: &mut zirc_irc::multiline::Batches,
    matches: impl Fn(&Line) -> bool,
) -> Line {
    tokio::time::timeout(WAIT, async {
        loop {
            let line = incoming.recv().await.expect("still connected");
            if let Some(line) = batches.feed(line)
                && matches(&line)
            {
                return line;
            }
        }
    })
    .await
    .expect("no matching message in time")
}

/// The next line with `command` from the server, skipping others.
///
/// # Panics
///
/// As [`next_matching`] does.
pub async fn expect(incoming: &mut Incoming, command: &str) -> Line {
    next_matching(incoming, |line| line.command == command).await
}

/// An integration's client on `ergo`, such as CI's (`zirc-ci`): its account registered and
/// signed in to, so the server says whose its lines are (`account-tag`), which is what
/// zircd takes CI's word by. Once a test for each `nick`.
///
/// # Panics
///
/// When it can't register or connect.
pub async fn integration(ergo: &Ergo, nick: &str) -> (Client, Incoming) {
    let passphrase = "integration-passphrase";
    register(ergo, nick, passphrase).await;
    let config = Config {
        sasl: Some(zirc_irc::Credentials {
            account: nick.to_owned(),
            password: passphrase.to_owned(),
        }),
        ..config(ergo, nick)
    };
    zirc_irc::connect(&config).await.expect("signed in to Ergo")
}

/// Makes the registered account `account` one of `owner`'s machines, as a tenant's sidecar does:
/// its host, `machine/<owner>`, set by the server's operator ([`zirc_test_ergo::OPER`]).
///
/// # Panics
///
/// When the operator isn't let in or `HostServ` doesn't say it set the host.
pub async fn host_machine(ergo: &Ergo, account: &str, owner: &str) {
    set_host(ergo, account, &format!("machine/{}", owner.to_lowercase())).await;
}

/// Makes the registered account `account` `owner`'s own client under another name, such as
/// their IRC client, as a tenant's sidecar does: its host, `client/<owner>`.
///
/// # Panics
///
/// When the operator isn't let in or `HostServ` doesn't say it set the host.
pub async fn host_client(ergo: &Ergo, account: &str, owner: &str) {
    set_host(ergo, account, &format!("client/{}", owner.to_lowercase())).await;
}

/// Sets `account`'s host to `host`, as the server's operator.
async fn set_host(ergo: &Ergo, account: &str, host: &str) {
    let (client, mut incoming) = person(ergo, "oper-sets-host").await;
    let (name, password) = zirc_test_ergo::OPER;
    client.send(Line::new("OPER", [name, password])).unwrap();
    expect(&mut incoming, "381").await;
    client
        .privmsg("HostServ", &format!("SET {account} {host}"))
        .unwrap();
    let notice = expect(&mut incoming, "NOTICE").await;
    assert!(
        notice
            .params
            .last()
            .unwrap()
            .to_lowercase()
            .contains("success"),
        "{notice:?}"
    );
    client.quit("hosted").unwrap();
}

/// Registers the account `name` with `passphrase`, as zirc.dev does.
///
/// # Panics
///
/// When the server doesn't say it's created.
pub async fn register(ergo: &Ergo, name: &str, passphrase: &str) {
    let (client, mut incoming) = person(ergo, name).await;
    client
        .privmsg("NickServ", &format!("REGISTER {passphrase}"))
        .unwrap();
    let notice = expect(&mut incoming, "NOTICE").await;
    assert!(
        notice.params.last().unwrap().contains("created"),
        "{notice:?}"
    );
    client.quit("registered").unwrap();
    // The nick is the account's from now on; let the registering client's go first.
    tokio::time::sleep(Duration::from_millis(300)).await;
}
