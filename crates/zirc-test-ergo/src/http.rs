//! The little HTTP this crate's stand-ins speak ([`crate::signin`], [`crate::control`]): one
//! request a connection, read whole, and one answer.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;

use serde_json::json;

/// One answer: its status, its content type, any `Location`, and its body.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) kind: &'static str,
    pub(crate) location: Option<String>,
    pub(crate) body: Vec<u8>,
}

impl Answer {
    pub(crate) fn json(status: u16, body: &impl serde::Serialize) -> Self {
        Self {
            status,
            kind: "application/json",
            location: None,
            body: serde_json::to_vec(body).unwrap_or_default(),
        }
    }

    pub(crate) fn error(status: u16, why: &str) -> Self {
        Self::json(status, &json!({ "error": why }))
    }

    pub(crate) fn file(kind: &'static str, body: Vec<u8>) -> Self {
        Self {
            status: 200,
            kind,
            location: None,
            body,
        }
    }
}

/// Writes `answer` to `stream`, which then closes.
pub(crate) fn respond(stream: &mut TcpStream, answer: Answer) {
    let location = answer
        .location
        .map(|to| format!("location: {to}\r\n"))
        .unwrap_or_default();
    let head = format!(
        "HTTP/1.1 {} X\r\ncontent-type: {}\r\ncontent-length: {}\r\n{location}connection: close\r\n\r\n",
        answer.status,
        answer.kind,
        answer.body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&answer.body);
}

/// A request's head and its body, once all of it has come.
pub(crate) fn read_request(stream: &mut TcpStream) -> Option<(String, String)> {
    let mut request = Vec::new();
    let mut buf = [0; 4096];
    loop {
        let n = stream.read(&mut buf).ok()?;
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
                return Some((head.to_owned(), rest.to_owned()));
            }
        }
        if n == 0 {
            return None;
        }
    }
}
