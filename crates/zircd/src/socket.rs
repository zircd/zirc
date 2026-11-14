//! The Unix socket `zirc` commands talk to. Each connection carries one request; a watch
//! request keeps it open to stream deliveries.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use tokio::io::{AsyncReadExt as _, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};
use zirc_core::ipc::{self, Addressed, Request, Response};

use crate::router::Call;

/// How many messages a watching client can fall behind before it misses some: the socket's
/// client behind the router, and the router behind each organisation's daemon.
pub(crate) const WATCH_BUFFER: usize = 256;

/// What a request is told when zircd has stopped taking them, as while it restarts.
pub(crate) const STOPPING: &str = "zircd is stopping";

/// How long a client may take to send its request once it's connected: `zirc` sends it at
/// once, so one that sends nothing for this long is stuck or isn't zirc, and is hung up on.
const REQUEST_WITHIN: Duration = Duration::from_secs(30);

/// Locks `home` for this zircd, for as long as the file it returns is open, with this
/// process's id and a token of its own in it (`zirc_core::paths::lock_token`). A second
/// zircd on the same home is refused here, before it touches the socket: without the lock,
/// two starting at once could each take the other's socket for a stale one and remove it.
///
/// A zircd that stops and starts again at once can find the lock still held for a moment: a
/// process forked just then by something else in the same process holds a copy of the lock's
/// file until it execs (the tests start Ergo and git beside zircd, and flaked on it), so the
/// lock is tried for [`LOCK_GRACE`] before the other is taken to be running.
pub async fn lock(home: &Path) -> Result<File> {
    lock_within(home, LOCK_GRACE).await
}

/// How long [`lock`] waits for a lock that is held, in case its holder is on its way out.
const LOCK_GRACE: Duration = Duration::from_secs(3);

/// [`lock`], waiting `grace` for a lock that is held.
async fn lock_within(home: &Path, grace: Duration) -> Result<File> {
    let path = zirc_core::paths::lock(home);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    let until = tokio::time::Instant::now() + grace;
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if tokio::time::Instant::now() < until => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(TryLockError::WouldBlock) => {
                let mut pid = String::new();
                let _ = file.read_to_string(&mut pid);
                bail!(
                    "zircd is already running (process {}, {})",
                    pid.split_whitespace().next().unwrap_or_default(),
                    home.display()
                );
            }
            Err(TryLockError::Error(err)) => {
                return Err(err).with_context(|| format!("locking {}", path.display()));
            }
        }
    }
    file.set_len(0)?;
    writeln!(file, "{} {}", std::process::id(), token())?;
    Ok(file)
}

/// A token no other zircd has had: the time to the nanosecond and a count of this process's.
fn token() -> String {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!(
        "{nanos}-{}",
        COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// Listens on `path`, replacing a socket left behind by a zircd that's gone, but refusing
/// to take over from one that's still running. Called holding [`lock`].
pub async fn bind(path: &Path) -> Result<UnixListener> {
    zirc_core::paths::check_socket(path)?;
    if path.exists() {
        if UnixStream::connect(path).await.is_ok() {
            bail!("zircd is already running ({})", path.display());
        }
        std::fs::remove_file(path).context("removing a stale socket")?;
    }
    let listener =
        UnixListener::bind(path).with_context(|| format!("listening on {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Accepts connections until the daemon goes away.
pub async fn serve(listener: UnixListener, inputs: mpsc::UnboundedSender<Call>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let inputs = inputs.clone();
                tokio::spawn(async move {
                    if let Err(err) = client(stream, inputs, REQUEST_WITHIN).await {
                        tracing::debug!("socket client: {err:#}");
                    }
                });
            }
            Err(err) => tracing::warn!("accepting a socket client failed: {err}"),
        }
    }
}

/// Serves one connection: its request, read within `within`, and the answer.
async fn client(
    stream: UnixStream,
    inputs: mpsc::UnboundedSender<Call>,
    within: Duration,
) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    let Ok(received) = tokio::time::timeout(within, ipc::receive_request(&mut read)).await else {
        let message = format!("no request within {}s", within.as_secs());
        ipc::send(&mut write, &Response::Error { message }).await?;
        return Ok(());
    };
    let request = match received {
        Ok(Some(request)) => request,
        Ok(None) => return Ok(()),
        Err(err @ ipc::Error::Protocol(_)) => {
            let message = err.to_string();
            ipc::send(&mut write, &Response::Error { message }).await?;
            return Ok(());
        }
        Err(err) => {
            let message = format!("not a request: {err}");
            ipc::send(&mut write, &Response::Error { message }).await?;
            return Ok(());
        }
    };

    if let Addressed {
        request: Request::Watch { session },
        ..
    } = request
    {
        let (heard, mut receive) = mpsc::channel(WATCH_BUFFER);
        inputs.send(Call::Watch(heard, session))?;
        // Keep reading: the client sends nothing more, so its end of the stream means it's
        // gone, and dropping `receive` then tells the daemon at once, not at the next write.
        let mut hung_up = [0u8; 64];
        loop {
            tokio::select! {
                response = receive.recv() => match response {
                    Some(response) => ipc::send(&mut write, &response).await?,
                    None => return Ok(()),
                },
                read = read.read(&mut hung_up) => {
                    if !read.is_ok_and(|read| read > 0) {
                        return Ok(());
                    }
                }
            }
        }
    }

    let (answer, response) = oneshot::channel();
    // A daemon that has stopped taking requests, as one restarting, still answers: a
    // connection closed without a word reads as a broken zircd.
    let stopping = || Response::Error {
        message: STOPPING.to_owned(),
    };
    let response = match inputs.send(Call::Request(Box::new(request), answer)) {
        Ok(()) => response.await.unwrap_or_else(|_| stopping()),
        Err(_) => stopping(),
    };
    ipc::send(&mut write, &response).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt as _;

    use super::*;

    #[tokio::test]
    async fn the_lock_waits_for_a_holder_on_its_way_out_and_then_refuses() {
        let home = tempfile::tempdir().unwrap();
        let held = lock_within(home.path(), Duration::ZERO).await.unwrap();
        let refused = lock_within(home.path(), Duration::from_millis(100)).await;
        assert!(refused.unwrap_err().to_string().contains("already running"));
        let letting_go = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            drop(held);
        });
        let taken = lock_within(home.path(), Duration::from_secs(5)).await;
        assert!(taken.is_ok(), "{:?}", taken.err());
        letting_go.await.unwrap();
    }

    /// What [`client`] answers once `send` has been written to it, and whether the request
    /// reached the daemon, which answers [`Response::Done`].
    async fn answer(send: &[u8], within: Duration) -> (Response, bool) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let (calls, mut called) = mpsc::unbounded_channel();
        let serving = tokio::spawn(client(theirs, calls, within));
        let (read, mut write) = ours.into_split();
        write.write_all(send).await.unwrap();
        // `None` once the client's served without asking the daemon anything.
        let reached = match called.recv().await {
            Some(Call::Request(_, answer)) => answer.send(Response::Done).is_ok(),
            _ => false,
        };
        let response = ipc::receive(&mut BufReader::new(read))
            .await
            .unwrap()
            .unwrap();
        serving.await.unwrap().unwrap();
        (response, reached)
    }

    #[tokio::test]
    async fn a_client_that_sends_nothing_is_hung_up_on() {
        let (response, reached) = answer(b"", Duration::from_millis(100)).await;
        assert!(!reached, "nothing reached the daemon");
        assert!(
            matches!(&response, Response::Error { message } if message.starts_with("no request within")),
            "{response:?}"
        );
    }

    #[tokio::test]
    async fn a_half_sent_request_is_hung_up_on_too() {
        let (response, reached) = answer(br#"{"op":"sta"#, Duration::from_millis(100)).await;
        assert!(!reached);
        assert!(
            matches!(&response, Response::Error { message } if message.starts_with("no request within")),
            "{response:?}"
        );
    }

    #[tokio::test]
    async fn a_request_in_time_is_answered() {
        let (response, reached) = answer(b"{\"op\":\"status\"}\n", Duration::from_secs(5)).await;
        assert!(reached);
        assert_eq!(response, Response::Done);
    }

    #[tokio::test]
    async fn a_request_past_the_cap_gets_a_protocol_error() {
        let long = usize::try_from(ipc::MAX_REQUEST_BYTES).unwrap() + 1;
        let (ours, theirs) = UnixStream::pair().unwrap();
        let (calls, _called) = mpsc::unbounded_channel();
        let serving = tokio::spawn(client(theirs, calls, Duration::from_secs(30)));
        let (read, mut write) = ours.into_split();
        // Written apart from the reading, as the socket's buffer holds far less than it.
        let writing = tokio::spawn(async move {
            let _ = write.write_all(&vec![b' '; long]).await;
            write
        });
        let response: Response = ipc::receive(&mut BufReader::new(read))
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(&response, Response::Error { message } if message.contains("longer than")),
            "{response:?}"
        );
        serving.await.unwrap().unwrap();
        drop(writing.await);
    }
}
