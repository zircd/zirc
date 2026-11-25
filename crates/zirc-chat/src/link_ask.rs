//! Links an app asks about before it opens them. A link in a message is whatever its sender
//! wrote: one that isn't a web address (`file:`, `smb:`, another app's own scheme) is handed
//! to the system to do as it likes with, and one whose words are themselves an address can
//! show one site and open another. Those two an app asks about first, in these words (the
//! user's choice, 13 Nov 2026); every other web address and `mailto:` opens as it always has.
//!
//! An app draws a link at [`guarded`]'s address, which for one to ask about is under
//! [`SCHEME`], an address only this module reads: tapped, the app gets [`asked`]'s question
//! for it and opens [`Ask::address`] on a yes. So no app decides which links to ask about.

/// The scheme of a link an app asks about before opening, as [`guarded`] writes it.
pub const SCHEME: &str = "zirc-ask";

/// The button that opens the link once asked.
pub const OPEN: &str = "Open";

/// The most of an address a question shows, in characters.
const SHOWN: usize = 120;

/// What an app asks before it opens a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    /// The question, over [`OPEN`] and Cancel.
    pub prompt: String,
    /// Where the link goes: what the app opens on a yes.
    pub address: String,
}

/// The address an app gives the link drawn as `text` that goes to `url`: `url` itself for one
/// that opens at once, or one under [`SCHEME`] to ask about first.
#[must_use]
pub fn guarded(text: &str, url: &str) -> String {
    match why(text, url) {
        Some(says) => format!("{SCHEME}:{}?says={}", hex(url), hex(&says)),
        None => url.to_owned(),
    }
}

/// The question for `link`, an address [`guarded`] wrote; none for any other.
#[must_use]
pub fn asked(link: &str) -> Option<Ask> {
    let (address, says) = link
        .strip_prefix(SCHEME)?
        .strip_prefix(':')?
        .split_once("?says=")?;
    let (address, says) = (unhex(address)?, unhex(says)?);
    let shown: String = address.chars().take(SHOWN).collect();
    let cut = shown.len() < address.len();
    let more = if cut { "…" } else { "" };
    let prompt = if says.is_empty() {
        format!(
            "This isn't a web link, and may open another app or a file on this device. Open {shown}{more}?"
        )
    } else {
        format!(
            "This link says {says} but goes to {}. Open {shown}{more}?",
            host(&address).unwrap_or_default()
        )
    };
    Some(Ask { prompt, address })
}

/// Why the link drawn as `text` to `url` is asked about, if it is: the site its words name
/// when that isn't where it goes, or nothing said (an empty string) for one that isn't a web
/// address at all.
fn why(text: &str, url: &str) -> Option<String> {
    let scheme = url.split_once(':').map(|(scheme, _)| scheme);
    let scheme = scheme.map(str::to_ascii_lowercase);
    match scheme.as_deref() {
        Some("http" | "https") => {
            let (said, goes) = (host(text.trim())?, host(url)?);
            (said != goes).then_some(said)
        }
        Some("mailto") => None,
        _ => Some(String::new()),
    }
}

/// The site an address names, `zirc.dev` for `https://www.zirc.dev/login`, in lower case and
/// without `www.`: for a web address, or words written as one, with or without `https://`.
/// None for anything else, such as a link's ordinary words.
fn host(address: &str) -> Option<String> {
    let rest = ["https://", "http://"]
        .iter()
        .find_map(|scheme| {
            let start = address.get(..scheme.len())?;
            start
                .eq_ignore_ascii_case(scheme)
                .then(|| &address[scheme.len()..])
        })
        .unwrap_or(address);
    if rest.chars().any(char::is_whitespace) {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    // Before an `@` is who signs in, never the site.
    let site = rest[..end].rsplit('@').next()?;
    let site = site.split(':').next()?.to_ascii_lowercase();
    let named = site.contains('.')
        && site
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    named.then(|| site.strip_prefix("www.").unwrap_or(&site).to_owned())
}

fn hex(text: &str) -> String {
    use std::fmt::Write as _;
    text.bytes().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

fn unhex(hex: &str) -> Option<String> {
    String::from_utf8(zirc_proto::unhex(hex)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_addresses_and_mail_open_as_they_are() {
        for (text, url) in [
            ("the docs", "https://zirc.dev/docs"),
            ("zirc.dev/docs", "https://www.zirc.dev/docs"),
            ("https://zirc.dev", "https://ZIRC.dev/login"),
            ("mail me", "mailto:ada@example.com"),
            ("see v1.2 for it", "https://example.com/v1.2"),
        ] {
            assert_eq!(guarded(text, url), url, "{text}");
            assert_eq!(asked(url), None);
        }
    }

    #[test]
    fn a_link_that_isnt_a_web_address_is_asked_about() {
        let file = "file:///Applications/Calculator.app";
        let link = guarded("release notes", file);
        assert!(link.starts_with("zirc-ask:"), "{link}");
        let ask = asked(&link).unwrap();
        assert_eq!(ask.address, file);
        let prompt = ask.prompt;
        assert!(prompt.starts_with("This isn't a web link"), "{prompt}");
        assert!(prompt.ends_with("Open file:///Applications/Calculator.app?"));
        // Another app's scheme, a share, and one with no scheme at all.
        for other in ["smb://host/share", "slack://open", "notes"] {
            assert!(asked(&guarded("x", other)).is_some(), "{other}");
        }
    }

    #[test]
    fn words_that_name_one_site_and_go_to_another_are_asked_about() {
        let link = guarded("https://zirc.dev/login", "https://evil.example/zirc.dev");
        let ask = asked(&link).unwrap();
        assert_eq!(ask.address, "https://evil.example/zirc.dev");
        assert!(
            ask.prompt
                .starts_with("This link says zirc.dev but goes to evil.example."),
            "{}",
            ask.prompt
        );
        // Who signs in, before an @, isn't the site.
        assert!(asked(&guarded("zirc.dev", "https://zirc.dev@evil.example/")).is_some());
    }

    #[test]
    fn a_long_address_is_cut_in_the_question_and_only_its_own_addresses_are_read() {
        let long = format!("file:///{}", "a".repeat(300));
        let ask = asked(&guarded("x", &long)).unwrap();
        assert!(ask.prompt.ends_with("…?") && ask.prompt.len() < 300);
        assert_eq!(ask.address, long);
        for other in ["zirc-ask:zz?says=", "zirc-ask:6869", "https://zirc.dev"] {
            assert_eq!(asked(other), None, "{other}");
        }
    }
}
