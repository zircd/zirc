//! Links to a conversation, or to one message in it, that every app opens:
//! `https://zirc.dev/app/c/<org>/<channel>?m=<msgid>`.
//!
//! The phones open them where they're installed (universal links, app links); elsewhere
//! zirc.dev's page at that path opens the desktop app, as `zirc://c/<org>/<channel>?m=…`,
//! or else web chat at the same place. A channel's `#` is left out and a room's `/` kept:
//! `#monorepo/agents` is `…/c/zirc/monorepo/agents`. Each segment is percent-encoded.

use std::fmt::Write as _;

/// A conversation, and maybe one message in it, as a link names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    /// The organisation, by its slug.
    pub org: String,
    /// The channel, `#monorepo/agents`.
    pub channel: String,
    /// The message's server id (`msgid`), when the link is to one.
    pub message: Option<String>,
}

/// The link to `channel` in `org`, at message `message` if given, on `web`, zirc.dev's
/// address (`https://zirc.dev`, or another's). None for a target that isn't a channel,
/// such as a nick.
#[must_use]
pub fn conversation_link(
    web: &str,
    org: &str,
    channel: &str,
    message: Option<&str>,
) -> Option<String> {
    let path = path(org, channel)?;
    Some(format!(
        "{}/app{path}{}",
        web.trim_end_matches('/'),
        query(message)
    ))
}

/// The same place as [`conversation_link`], as the desktop app's own link, `zirc://c/…`.
#[must_use]
pub fn desktop_link(org: &str, channel: &str, message: Option<&str>) -> Option<String> {
    let path = path(org, channel)?;
    Some(format!("zirc:/{path}{}", query(message)))
}

/// The conversation `url` names: a link from [`conversation_link`] or [`desktop_link`]. None
/// for any other.
#[must_use]
pub fn parse(url: &str) -> Option<Conversation> {
    let rest = if let Some(rest) = url.strip_prefix("zirc://") {
        rest
    } else {
        let rest = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))?;
        let (_origin, path) = rest.split_once('/')?;
        path.strip_prefix("app/")?
    };
    let (rest, _fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let mut parts = path.trim_end_matches('/').split('/');
    if parts.next()? != "c" {
        return None;
    }
    let org = decode(parts.next()?)?;
    let channel = parts.map(decode).collect::<Option<Vec<_>>>()?;
    if org.is_empty() || channel.is_empty() || channel.iter().any(String::is_empty) {
        return None;
    }
    let message = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("m="))
        .and_then(decode)
        .filter(|m| !m.is_empty());
    Some(Conversation {
        org,
        channel: format!("#{}", channel.join("/")),
        message,
    })
}

/// `/c/<org>/<channel's segments>`, encoded.
fn path(org: &str, channel: &str) -> Option<String> {
    let name = channel.strip_prefix('#')?;
    if org.is_empty() || name.is_empty() || name.split('/').any(str::is_empty) {
        return None;
    }
    let segments: Vec<String> = name.split('/').map(encode).collect();
    Some(format!("/c/{}/{}", encode(org), segments.join("/")))
}

fn query(message: Option<&str>) -> String {
    message
        .map(|m| format!("?m={}", encode(m)))
        .unwrap_or_default()
}

/// Percent-encodes all but RFC 3986's unreserved characters.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// Undoes [`encode`]: None for a malformed escape or text that isn't UTF-8.
fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_a_room_and_a_message_and_reads_them_back() {
        let link = conversation_link(
            "https://zirc.dev/",
            "zirc",
            "#monorepo/agents",
            Some("01jbx7ab"),
        )
        .unwrap();
        assert_eq!(
            link,
            "https://zirc.dev/app/c/zirc/monorepo/agents?m=01jbx7ab"
        );
        let want = Conversation {
            org: "zirc".into(),
            channel: "#monorepo/agents".into(),
            message: Some("01jbx7ab".into()),
        };
        assert_eq!(parse(&link), Some(want.clone()));
        let desktop = desktop_link("zirc", "#monorepo/agents", Some("01jbx7ab")).unwrap();
        assert_eq!(desktop, "zirc://c/zirc/monorepo/agents?m=01jbx7ab");
        assert_eq!(parse(&desktop), Some(want));
    }

    #[test]
    fn a_channel_alone_and_odd_characters() {
        assert_eq!(
            conversation_link("https://zirc.example.com", "zirc", "#web", None).as_deref(),
            Some("https://zirc.example.com/app/c/zirc/web")
        );
        let odd = conversation_link("https://zirc.dev", "zirc", "#q&a?ü", Some("a+b")).unwrap();
        assert_eq!(odd, "https://zirc.dev/app/c/zirc/q%26a%3F%C3%BC?m=a%2Bb");
        assert_eq!(
            parse(&odd),
            Some(Conversation {
                org: "zirc".into(),
                channel: "#q&a?ü".into(),
                message: Some("a+b".into())
            })
        );
    }

    #[test]
    fn nothing_for_what_isnt_a_channel_link() {
        assert_eq!(
            conversation_link("https://zirc.dev", "zirc", "bob", None),
            None
        );
        assert_eq!(
            conversation_link("https://zirc.dev", "zirc", "#a//b", None),
            None
        );
        for url in [
            "https://zirc.dev/app/c/zirc",
            "https://zirc.dev/app/c//web",
            "https://zirc.dev/app/signed-in",
            "https://zirc.dev/c/zirc/web",
            "https://zirc.dev/app/c/zirc/w%zz",
            "zirc.dev/app/c/zirc/web",
        ] {
            assert_eq!(parse(url), None, "{url}");
        }
        assert_eq!(
            parse("https://zirc.dev/app/c/zirc/web?m=").unwrap().message,
            None
        );
    }
}
