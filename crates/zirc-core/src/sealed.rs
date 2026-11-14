//! Sealed parts of messages with the system's randomness: zirc-chat's [`zirc_chat::sealed`],
//! which the browser runs with its own, for the apps, zircd and the CLI.

use ring::rand::{SecureRandom as _, SystemRandom};
pub use zirc_chat::sealed::{
    Context, KEY_LEN, Key, KeyError, MARKER, NONCE_LEN, Opened, SealError, Split, open_message,
    split,
};

/// Why a key couldn't be made or parts sealed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The system gave no random bytes.
    #[error("the system gave no random bytes")]
    NoRandom,
    /// The parts couldn't be sealed.
    #[error("{0}")]
    Seal(#[from] SealError),
}

/// A new random key for `org`.
///
/// # Errors
///
/// [`Error::NoRandom`] when the system gives no random bytes.
pub fn new_key(org: impl Into<String>) -> Result<Key, Error> {
    Ok(Key::new(org, random()?))
}

/// The value of the `+zirc.dev/sealed` tag for `parts`, each sealed with `key` and a random
/// nonce. `room` is what's left of the message's client tags after its others.
///
/// # Errors
///
/// [`Error::Seal`] when they won't fit in `room`, [`Error::NoRandom`] when the system gives
/// no random bytes.
pub fn seal_message(
    key: &Key,
    context: &Context,
    parts: &[String],
    room: usize,
) -> Result<String, Error> {
    let mut nonces = Vec::with_capacity(parts.len());
    for _ in parts {
        nonces.push(random::<NONCE_LEN>()?);
    }
    let mut nonces = nonces.into_iter();
    Ok(zirc_chat::sealed::seal_message(
        key,
        context,
        parts,
        || nonces.next().unwrap_or_default(),
        room,
    )?)
}

fn random<const N: usize>() -> Result<[u8; N], Error> {
    let mut bytes = [0; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| Error::NoRandom)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use zirc_proto::sealed::{CLIENT_TAGS_MAX, Sealed};

    use super::*;

    const ACME: Context = Context {
        org: "acme",
        target: "#api",
        sender: "alice",
    };

    #[test]
    fn makes_a_different_key_each_time() {
        let key = new_key("acme").unwrap();
        assert_ne!(key, new_key("acme").unwrap());
        assert_eq!(key.org(), "acme");
        assert_eq!(Key::parse(&key.export()), Ok(key));
    }

    #[test]
    fn seals_each_part_with_its_own_nonce() {
        let key = new_key("acme").unwrap();
        let parts = vec!["same".to_owned(), "same".to_owned()];
        let value = seal_message(&key, &ACME, &parts, CLIENT_TAGS_MAX).unwrap();
        let sealed = Sealed::from_tag(&value).unwrap();
        assert_ne!(sealed.parts[0].n, sealed.parts[1].n);
        assert_eq!(
            open_message(&[key], &ACME, &value),
            Some(vec![
                Opened::Value("same".into()),
                Opened::Value("same".into())
            ])
        );
    }
}
