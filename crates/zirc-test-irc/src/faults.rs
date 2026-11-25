//! A proxy between clients and the test Ergo that breaks their connections the ways real
//! networks do: [`Proxy::reset`] as a router or server dropping them, [`Proxy::blackhole`]
//! as a laptop sleeping or the Wi-Fi changing (what's sent is lost and nothing says so),
//! and [`Proxy::refuse`] as a server that's down.

use tokio::io::AsyncWriteExt as _;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// What the proxy does with connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Passes everything on.
    Pass,
    /// Loses everything, both ways, on open connections and new ones alike. A connection it
    /// lost stays lost.
    Blackhole,
    /// Resets new connections as they come.
    Refuse,
}

/// A proxy on a loopback port of its own, to the server on `upstream`'s.
pub struct Proxy {
    port: u16,
    /// The mode, and how many times open connections were reset.
    state: watch::Sender<(Mode, u64)>,
    task: JoinHandle<()>,
}

impl Proxy {
    /// Starts passing connections on to the server on loopback port `upstream`.
    ///
    /// # Panics
    ///
    /// When it can't listen.
    pub async fn start(upstream: u16) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let port = listener.local_addr().expect("its address").port();
        let (state, watching) = watch::channel((Mode::Pass, 0));
        let task = tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                tokio::spawn(carry(client, upstream, watching.clone()));
            }
        });
        Self { port, state, task }
    }

    /// The port to connect to on 127.0.0.1.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Resets every open connection, as a server crashing or a router dropping them does:
    /// the client hears of it at once.
    pub fn reset(&self) {
        self.state.send_modify(|(_, resets)| *resets += 1);
    }

    /// Loses everything from now on, and says nothing: as a network does after a laptop
    /// sleeps or moves to another, until [`Proxy::heal`].
    pub fn blackhole(&self) {
        self.state.send_modify(|(mode, _)| *mode = Mode::Blackhole);
    }

    /// Resets connections as they come, as a server that's down: until [`Proxy::heal`].
    /// Open ones carry on.
    pub fn refuse(&self) {
        self.state.send_modify(|(mode, _)| *mode = Mode::Refuse);
    }

    /// Passes new connections on again. One already lost stays lost.
    pub fn heal(&self) {
        self.state.send_modify(|(mode, _)| *mode = Mode::Pass);
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Carries one client's connection, as the mode says, until it ends or is reset.
async fn carry(mut client: TcpStream, upstream: u16, mut state: watch::Receiver<(Mode, u64)>) {
    let (mode, resets) = *state.borrow_and_update();
    match mode {
        Mode::Refuse => return reset(client),
        Mode::Blackhole => return lose(client, None, state, resets).await,
        Mode::Pass => {}
    }
    let Ok(mut server) = TcpStream::connect(("127.0.0.1", upstream)).await else {
        return reset(client);
    };
    loop {
        tokio::select! {
            _ = tokio::io::copy_bidirectional(&mut client, &mut server) => {
                let _ = client.shutdown().await;
                return;
            }
            changed = state.changed() => {
                if changed.is_err() {
                    return;
                }
                let (mode, now) = *state.borrow_and_update();
                if now != resets {
                    reset(server);
                    return reset(client);
                }
                if mode == Mode::Blackhole {
                    return lose(client, Some(server), state, resets).await;
                }
            }
        }
    }
}

/// Holds a connection, reading nothing and saying nothing, until it's reset.
async fn lose(
    client: TcpStream,
    server: Option<TcpStream>,
    mut state: watch::Receiver<(Mode, u64)>,
    resets: u64,
) {
    while state.changed().await.is_ok() {
        if state.borrow_and_update().1 != resets {
            break;
        }
    }
    if let Some(server) = server {
        reset(server);
    }
    reset(client);
}

/// Closes `stream` with a reset rather than a goodbye.
fn reset(stream: TcpStream) {
    let _ = stream.set_zero_linger();
    drop(stream);
}

/// A proxy that holds back everything it carries by `one_way`, each way, as a network far
/// from the server does: a phone's is 30 to 100 ms. On loopback a round trip costs nothing,
/// so what a client waits for one after another shows only through this one, as a multiple
/// of the round trip.
pub struct Latency {
    port: u16,
    one_way: std::time::Duration,
    task: JoinHandle<()>,
}

/// The round trips a cold start may wait on, one after another, before every channel it's in
/// is caught up: registering (CAP, SASL, the welcome), then the channels' joins and histories,
/// sent together. With 13 channels it took the apps' core (zirc-app) 5.5 and the TUI 6.6 when
/// this was set (25 Nov 2026); one more for each channel asked after the last would be 19 and
/// over. Both their reconnect tests keep it ([`Latency::keeps_cold_start`]).
pub const COLD_START_ROUND_TRIPS: f64 = 10.0;

impl Latency {
    /// How far a phone's network is from the server, each way.
    pub const PHONE: std::time::Duration = std::time::Duration::from_millis(50);

    /// Starts carrying connections to the server on loopback port `upstream` as a phone's
    /// network would ([`Latency::PHONE`]).
    pub async fn phone(upstream: u16) -> Self {
        Self::start(upstream, Self::PHONE).await
    }

    /// How many of this network's round trips `took` is.
    #[must_use]
    pub fn round_trips(&self, took: std::time::Duration) -> f64 {
        took.as_secs_f64() / (self.one_way * 2).as_secs_f64()
    }

    /// Says how long `what`, a cold start, took over this network, and fails when that's
    /// more than [`COLD_START_ROUND_TRIPS`].
    ///
    /// # Panics
    ///
    /// When it took longer.
    pub fn keeps_cold_start(&self, what: &str, took: std::time::Duration) {
        let trips = self.round_trips(took);
        println!(
            "PERF {what}: caught up in {took:?}, {trips:.1} round trips of {:?}",
            self.one_way * 2
        );
        assert!(
            trips < COLD_START_ROUND_TRIPS,
            "{what} took {took:?}, {trips:.1} round trips: more than {COLD_START_ROUND_TRIPS}"
        );
    }

    /// Starts carrying connections to the server on loopback port `upstream`, `one_way` late.
    ///
    /// # Panics
    ///
    /// When it can't listen.
    pub async fn start(upstream: u16, one_way: std::time::Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let port = listener.local_addr().expect("its address").port();
        let task = tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let Ok(server) = TcpStream::connect(("127.0.0.1", upstream)).await else {
                        return reset(client);
                    };
                    let _ = client.set_nodelay(true);
                    let _ = server.set_nodelay(true);
                    let (from_client, to_client) = client.into_split();
                    let (from_server, to_server) = server.into_split();
                    tokio::join!(
                        late(from_client, to_server, one_way),
                        late(from_server, to_client, one_way),
                    );
                });
            }
        });
        Self {
            port,
            one_way,
            task,
        }
    }

    /// The port to connect to on 127.0.0.1.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for Latency {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Carries what `from` reads to `to`, each piece `one_way` after it was read, in order.
async fn late(
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    one_way: std::time::Duration,
) {
    use tokio::io::AsyncReadExt as _;
    let (pieces, mut due) =
        tokio::sync::mpsc::unbounded_channel::<(tokio::time::Instant, Vec<u8>)>();
    let reading = async move {
        let mut buffer = vec![0; 64 * 1024];
        while let Ok(n @ 1..) = from.read(&mut buffer).await {
            if pieces
                .send((tokio::time::Instant::now() + one_way, buffer[..n].to_vec()))
                .is_err()
            {
                break;
            }
        }
    };
    let writing = async move {
        while let Some((at, piece)) = due.recv().await {
            tokio::time::sleep_until(at).await;
            if to.write_all(&piece).await.is_err() {
                break;
            }
        }
        let _ = to.shutdown().await;
    };
    tokio::join!(reading, writing);
}
