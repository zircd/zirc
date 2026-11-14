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
