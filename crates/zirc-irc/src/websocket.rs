//! IRC over a WebSocket, as a guest gateway takes it: IRCv3's `text.ircv3.net` subprotocol,
//! each text message one line without its line break. A task bridges the socket to a byte
//! stream of `\r\n`-ended lines, so registration and the reader and writer are the ones a
//! TCP connection has.

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _, DuplexStream};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_util::codec::{FramedRead, LinesCodec};

use crate::client::{
    Client, Config, Error, Incoming, MAX_LINE_BYTES, register, server_name, tls_connector,
};

/// The subprotocol a line per text message is (IRCv3's WebSocket spec).
pub const SUBPROTOCOL: &str = "text.ircv3.net";

/// How much may wait between the socket and registration, each way.
const BRIDGE_BUFFER: usize = 64 * 1024;

/// Connects to the WebSocket at `url` and registers over it as `config` says.
pub async fn connect(url: &str, config: &Config) -> Result<(Client, Incoming), Error> {
    let tls = if url.starts_with("wss://") {
        true
    } else if url.starts_with("ws://") {
        false
    } else {
        return Err(unusable("a WebSocket's address starts ws:// or wss://"));
    };
    let mut request = url.into_client_request().map_err(failed)?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static(SUBPROTOCOL),
    );
    let uri = request.uri().clone();
    let host = uri
        .host()
        .ok_or_else(|| unusable("a WebSocket's address names a host"))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
    let tcp = TcpStream::connect((host.as_str(), port)).await?;
    tcp.set_nodelay(true)?;
    let ours = if tls {
        let stream = tls_connector()?.connect(server_name(&host)?, tcp).await?;
        bridged(request, stream).await?
    } else {
        bridged(request, tcp).await?
    };
    register(ours, config).await
}

/// Opens the WebSocket over `stream` and bridges it to the stream answered.
async fn bridged<S>(
    request: tungstenite::handshake::client::Request,
    stream: S,
) -> Result<DuplexStream, Error>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (socket, _response) = tokio_tungstenite::client_async(request, stream)
        .await
        .map_err(failed)?;
    let (ours, theirs) = tokio::io::duplex(BRIDGE_BUFFER);
    tokio::spawn(bridge(socket, theirs));
    Ok(ours)
}

/// Passes lines between `socket` and `lines` until either ends, then ends the other.
async fn bridge<S>(socket: WebSocketStream<S>, lines: DuplexStream)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut to_server, mut from_server) = socket.split();
    let (read, mut write) = tokio::io::split(lines);
    let mut outgoing = FramedRead::new(read, LinesCodec::new_with_max_length(MAX_LINE_BYTES));
    loop {
        tokio::select! {
            message = from_server.next() => {
                let text = match message {
                    Some(Ok(Message::Text(text))) => text.to_string(),
                    Some(Ok(Message::Binary(bytes))) => String::from_utf8_lossy(&bytes).into_owned(),
                    Some(Ok(Message::Close(_))) | None => break,
                    // Pings are answered by tungstenite itself.
                    Some(Ok(_)) => continue,
                    Some(Err(err)) => {
                        tracing::debug!("the WebSocket failed: {err}");
                        break;
                    }
                };
                // One line per message, but a server may send several.
                for line in text.split(['\r', '\n']).filter(|line| !line.is_empty()) {
                    if write.write_all(format!("{line}\r\n").as_bytes()).await.is_err() {
                        return;
                    }
                }
            }
            line = outgoing.next() => {
                let Some(Ok(line)) = line else { break };
                if let Err(err) = to_server.send(Message::text(line)).await {
                    tracing::debug!("writing to the WebSocket failed: {err}");
                    return;
                }
            }
        }
    }
    let _ = to_server.send(Message::Close(None)).await;
    let _ = write.shutdown().await;
}

/// An address that can't be connected to, as a failed connection.
fn unusable(why: &'static str) -> Error {
    Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, why))
}

/// A WebSocket's failure, as the connection's: [`Error::Io`], its cause the source.
fn failed(err: tungstenite::Error) -> Error {
    match err {
        tungstenite::Error::Io(err) => Error::Io(err),
        err => Error::Io(std::io::Error::other(err)),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

    use super::*;
    use crate::Line;

    /// Takes a guest's WebSocket, in the subprotocol it asked for.
    #[expect(
        clippy::result_large_err,
        clippy::unnecessary_wraps,
        reason = "tungstenite's callback's type"
    )]
    fn answer(request: &Request, mut response: Response) -> Result<Response, ErrorResponse> {
        assert_eq!(request.uri().path(), "/guest");
        let asked = request.headers().get("Sec-WebSocket-Protocol").cloned();
        assert_eq!(
            asked.as_ref().and_then(|v| v.to_str().ok()),
            Some(SUBPROTOCOL)
        );
        response
            .headers_mut()
            .insert("Sec-WebSocket-Protocol", asked.unwrap());
        Ok(response)
    }

    /// A server that takes one WebSocket, answering the subprotocol asked for, and hands
    /// it over.
    async fn server() -> (String, tokio::task::JoinHandle<WebSocketStream<TcpStream>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/guest", listener.local_addr().unwrap());
        let accepted = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            tokio_tungstenite::accept_hdr_async(tcp, answer)
                .await
                .unwrap()
        });
        (url, accepted)
    }

    async fn next_text(socket: &mut WebSocketStream<TcpStream>) -> String {
        loop {
            match tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .unwrap()
            {
                Some(Ok(Message::Text(text))) => return text.to_string(),
                Some(Ok(_)) => {}
                other => panic!("no line: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn registers_and_talks_a_line_per_text_message() {
        let (url, accepted) = server().await;
        let mut config = Config::new("ignored", 1, "me");
        config.caps.clear();
        config.password_maker = Some(std::sync::Arc::new(|| Some("grant proof".into())));
        let connecting = tokio::spawn({
            let config = config.clone();
            async move { connect(&url, &config).await }
        });
        let mut server = accepted.await.unwrap();

        // Its opening, each line a message of its own without a line break.
        let mut opening = Vec::new();
        while !opening
            .iter()
            .any(|line: &String| line.starts_with("USER "))
        {
            let text = next_text(&mut server).await;
            assert!(!text.contains(['\r', '\n']), "{text:?}");
            opening.push(text);
        }
        assert!(
            opening.contains(&"PASS :grant proof".to_owned()),
            "{opening:?}"
        );
        // The gateway names the terminal itself; two lines in one message are both read.
        server
            .send(Message::text(
                ":gw 001 guest-1-mbp :welcome\r\n:gw 422 guest-1-mbp :no motd",
            ))
            .await
            .unwrap();
        let (client, mut incoming) = connecting.await.unwrap().unwrap();
        assert_eq!(client.nick(), "guest-1-mbp");
        let motd = incoming.recv().await.unwrap();
        assert_eq!(motd.command, "422");

        client.privmsg("#web/guest-1", "hello there").unwrap();
        assert_eq!(
            next_text(&mut server).await,
            "PRIVMSG #web/guest-1 :hello there"
        );
        server
            .send(Message::text(":ada!a@h PRIVMSG #web/guest-1 :hi"))
            .await
            .unwrap();
        let heard: Line = incoming.recv().await.unwrap();
        assert_eq!(heard.params, ["#web/guest-1", "hi"]);
        // PINGs are answered over it too.
        server.send(Message::text("PING :x")).await.unwrap();
        assert_eq!(next_text(&mut server).await, "PONG x");

        // The server going away ends the connection.
        server.close(None).await.unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(5), incoming.recv()).await;
        assert!(matches!(ended, Ok(None)), "{ended:?}");
    }

    #[tokio::test]
    async fn a_refused_handshake_is_a_failed_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/guest", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let _ = tcp
                .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n")
                .await;
        });
        let config = Config::new("ignored", 1, "me");
        let err = connect(&url, &config).await.unwrap_err();
        assert!(matches!(err, Error::Io(_)), "{err:?}");
        let refused = connect("https://x.test/guest", &config).await.unwrap_err();
        assert!(matches!(refused, Error::Io(_)), "{refused:?}");
    }
}
