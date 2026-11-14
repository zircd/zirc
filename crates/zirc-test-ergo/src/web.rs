//! A stand-in for zirc.dev's web app, for tests of the clients that log in there.

use std::io::{Read, Write as _};
use std::net::{Ipv4Addr, TcpListener};
use std::thread::JoinHandle;

/// A web app on a free loopback port that answers each request, one connection each, with
/// the next of `responses`, an HTTP status and a JSON body, and then stops. It answers on a
/// thread of its own, so it serves async tests and plain ones alike.
///
/// Answers the web app's address, such as `http://127.0.0.1:4711`, and a handle that joins
/// to the requests it got, headers and bodies, in order.
///
/// # Panics
///
/// When no port is free; joining the handle panics when a request can't be read.
#[must_use]
pub fn serve(responses: Vec<(u16, String)>) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a free port");
    let web = format!("http://{}", listener.local_addr().expect("its address"));
    let served = std::thread::spawn(move || {
        responses
            .into_iter()
            .map(|(status, body)| {
                let (mut stream, _) = listener.accept().expect("a request");
                let request = read_request(&mut stream);
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("answering");
                request
            })
            .collect()
    });
    (web, served)
}

/// The headers, then as much body as they say.
fn read_request(stream: &mut impl Read) -> String {
    let mut request = Vec::new();
    let mut buf = [0; 4096];
    loop {
        let n = stream.read(&mut buf).expect("reading a request");
        request.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&request);
        if let Some((head, rest)) = text.split_once("\r\n\r\n") {
            let length = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|n| n.trim().parse().ok())
                })
                .unwrap_or(0);
            if rest.len() >= length {
                return text.into_owned();
            }
        }
        assert!(n > 0, "the request ended early:\n{text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_requests_in_turn_and_hands_them_over() {
        let (web, served) = serve(vec![(200, "{}".into()), (404, String::new())]);
        let address = web.trim_start_matches("http://");
        for (path, expected) in [("/a", "200"), ("/b", "404")] {
            let mut stream = std::net::TcpStream::connect(address).unwrap();
            write!(
                stream,
                "POST {path} HTTP/1.1\r\ncontent-length: 2\r\n\r\nhi"
            )
            .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            assert!(
                response.starts_with(&format!("HTTP/1.1 {expected} ")),
                "{response}"
            );
        }
        let requests = served.join().unwrap();
        assert!(requests[0].starts_with("POST /a ") && requests[0].ends_with("\r\n\r\nhi"));
        assert!(requests[1].starts_with("POST /b "));
    }
}
