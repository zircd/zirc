//! Updating zirc to its latest release, where zirc's installer put it.
//!
//! The installer (`release/install.sh`) leaves [`RECEIPT`] beside the binaries, naming the
//! releases it installed from. Only binaries with one update themselves: a Cargo or Bazel
//! build, or a package manager's, is updated the way it was installed.
//!
//! An update reads the releases' `latest.txt`, downloads that version's archive for this
//! platform, checks it against its published SHA-256, signed with zirc's release key
//! ([`RELEASE_KEY`]), and replaces `zirc` and `zircd` one by one, each written beside the old
//! one and renamed over it, so a running program keeps its own file and nothing ever sees
//! half a binary.
//!
//! Releases come over HTTPS only. Each archive's `.sha256` has a `.sha256.sig` beside it: an
//! Ed25519 signature of the `.sha256` file's bytes, in hex, which `release_sign` (in
//! `bin/release_sign.rs`) makes as a release is published, with the private key kept in GCP
//! Secret Manager. A release without one, or with one the key didn't make, isn't installed,
//! so whoever can write the releases' bucket still can't run code on the machines that update
//! from it. Releases before 0.1.33 weren't signed: the clients that installed them don't
//! check, and take the first signed release as any other.
//!
//! For tests, [`TEST_KEY`] in the environment names another public key to trust in place of
//! zirc's, and lets releases come from `file://` URLs too.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;
use zirc_proto::hex;

use serde::{Deserialize, Serialize};
use zirc_proto::PROGRAM;

/// The file the installer writes beside the binaries it installs.
pub const RECEIPT: &str = ".zirc-install.toml";

/// How to install zirc's release, for binaries that didn't come from it.
pub const REINSTALL: &str = zirc_proto::onboarding::INSTALL_COMMAND;

/// zirc's release key: the Ed25519 public key that signs every release's checksums, in hex.
/// Its private key is kept in a secret manager, which only publishing a release reads
/// (the monorepo's `release/README.md` says where).
pub const RELEASE_KEY: &str = "7da026b48ec6c4b8e0fc6d8bf25714dafd0f98628d329c2d4a811aacf3a6affa";

/// The environment variable that names, for tests only, another public key (in hex) to trust
/// in place of [`RELEASE_KEY`], and allows `file://` releases.
pub const TEST_KEY: &str = "ZIRC_UPDATE_TEST_KEY";

/// The binaries a release has: zirc, zircd, and zirc-remote, the host that serves a config's
/// commands to its owner (docs/remote.md). release/BUILD.bazel packs them and
/// release/install.sh installs them: change all three.
const BINARIES: [&str; 3] = ["zirc", "zircd", "zirc-remote"];

/// Binaries older releases had, removed on updating: `zirc tui` took `zirc-tui`'s place.
const RETIRED: [&str; 1] = ["zirc-tui"];

/// Why an update failed, or why these binaries don't update themselves.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Not where zirc's installer put it, as a Cargo or Bazel build, or a package manager's.
    #[error(
        "{} wasn't installed by {PROGRAM}'s installer, so it doesn't update itself: update it \
         the way you installed it, or install {PROGRAM}'s release with: {REINSTALL}",
        .0.display()
    )]
    NotInstalled(PathBuf),
    /// Where zirc's installer put it, but this account can't change it.
    #[error(
        "this account can't write to {}, so {PROGRAM} can't update itself there: install again \
         as someone who can, or somewhere you can write, with: {REINSTALL}",
        .0.display()
    )]
    NotWritable(PathBuf),
    /// The receipt couldn't be read.
    #[error("{path}: {problem}", path = .0.display(), problem = .1)]
    Receipt(PathBuf, String),
    /// A download failed.
    #[error("couldn't download {0}: {1}")]
    Download(String, String),
    /// What was downloaded isn't what was published.
    #[error("{0} doesn't match its SHA-256: expected {1}, got {2}")]
    Checksum(String, String, String),
    /// The checksum has no signature, or one zirc's release key didn't make.
    #[error(
        "{0}: {1}, so {PROGRAM} won't install it: it installs only releases signed with its release key"
    )]
    Signature(String, String),
    /// Releases from somewhere other than HTTPS.
    #[error(
        "{0}: {PROGRAM} updates only from https:// URLs: install again from zirc's releases with: \
         {REINSTALL}"
    )]
    Insecure(String),
    /// The release says something unreadable, or lacks a binary.
    #[error("{0}")]
    Release(String),
    /// Unpacking or replacing the binaries failed.
    #[error("{0}: {1}")]
    Io(String, std::io::Error),
}

/// A release's version, `major.minor.patch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    /// The version of this build.
    ///
    /// # Panics
    ///
    /// Never: the workspace's version is `major.minor.patch`.
    #[must_use]
    pub fn current() -> Self {
        env!("CARGO_PKG_VERSION")
            .parse()
            .expect("zirc's version is major.minor.patch")
    }
}

impl FromStr for Version {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Error> {
        let text = text.trim();
        let numbers: Vec<u64> = text
            .trim_start_matches('v')
            .split('.')
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|_| Error::Release(format!("{text:?} isn't a version")))?;
        match numbers[..] {
            [major, minor, patch] => Ok(Self(major, minor, patch)),
            _ => Err(Error::Release(format!("{text:?} isn't a version"))),
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// This platform, as release archives name it: `<os>-<arch>`.
#[must_use]
pub fn target() -> String {
    let os = std::env::consts::OS;
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    format!("{os}-{arch}")
}

/// What the installer says in [`RECEIPT`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    /// Where releases are published, as `ZIRC_DOWNLOAD_URL` was for the installer.
    releases: String,
}

/// What's said of a newer release, before what to do about it, as `name` calls itself:
/// "zirc 1.2.3 is out" from the program ([`zirc_proto::PROGRAM`]), "Zirc 1.2.3 is out" from
/// an app ([`zirc_proto::NAME`]).
#[must_use]
pub fn released(name: &str, version: impl fmt::Display) -> String {
    format!("{name} {version} is out")
}

/// Binaries zirc's installer put in a directory this account can write to.
#[derive(Debug, Clone)]
pub struct Install {
    dir: PathBuf,
    releases: String,
    trust: Trust,
}

/// Whose signatures an update takes, and from where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trust {
    /// The Ed25519 public key releases are signed with.
    key: [u8; 32],
    /// Whether `file://` releases are taken too, as tests' are.
    local: bool,
}

impl Trust {
    /// zirc's release key, over HTTPS; or, when [`TEST_KEY`] is set, that key, from
    /// `file://` URLs too.
    ///
    /// # Errors
    ///
    /// When [`TEST_KEY`] isn't an Ed25519 public key in hex.
    ///
    /// # Panics
    ///
    /// Never: [`RELEASE_KEY`] is one.
    pub fn from_env() -> Result<Self, Error> {
        match std::env::var(TEST_KEY) {
            Ok(hex) => Ok(Self {
                key: public_key(&hex)
                    .ok_or_else(|| Error::Release(format!("{TEST_KEY} isn't a public key")))?,
                local: true,
            }),
            Err(_) => Ok(Self {
                key: public_key(RELEASE_KEY).expect("RELEASE_KEY is a public key"),
                local: false,
            }),
        }
    }

    /// The public key, in hex.
    #[must_use]
    pub fn key(&self) -> String {
        hex(&self.key)
    }

    /// Whether `url` is somewhere releases may come from.
    fn allows(&self, url: &str) -> bool {
        url.starts_with("https://") || (self.local && url.starts_with("file://"))
    }

    /// Whether `signature`, in hex, is this key's of `message`.
    fn verifies(&self, message: &[u8], signature: &str) -> bool {
        let Some(signature) = unhex(signature.trim()) else {
            return false;
        };
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, self.key)
            .verify(message, &signature)
            .is_ok()
    }
}

/// Signs `message` with `private`, an Ed25519 key as PKCS#8 in hex, as a release's checksums
/// are: the signature, in hex, and the key's public half, in hex.
///
/// # Errors
///
/// When `private` isn't an Ed25519 PKCS#8 key in hex.
pub fn sign(private: &str, message: &[u8]) -> Result<(String, String), Error> {
    use ring::signature::KeyPair as _;
    let key = unhex(private.trim())
        .and_then(|der| ring::signature::Ed25519KeyPair::from_pkcs8(&der).ok())
        .ok_or_else(|| Error::Release("that isn't an Ed25519 key in hex".into()))?;
    Ok((
        hex(key.sign(message).as_ref()),
        hex(key.public_key().as_ref()),
    ))
}

/// A new Ed25519 key: its private half as PKCS#8 in hex, and its public half in hex.
///
/// # Errors
///
/// When the system can't give random bytes.
pub fn generate() -> Result<(String, String), Error> {
    use ring::signature::KeyPair as _;
    let random = ring::rand::SystemRandom::new();
    let failed = || Error::Release("couldn't make a key".into());
    let der = ring::signature::Ed25519KeyPair::generate_pkcs8(&random).map_err(|_| failed())?;
    let key = ring::signature::Ed25519KeyPair::from_pkcs8(der.as_ref()).map_err(|_| failed())?;
    Ok((hex(der.as_ref()), hex(key.public_key().as_ref())))
}

/// `hex` as an Ed25519 public key, if it is one.
fn public_key(hex: &str) -> Option<[u8; 32]> {
    unhex(hex.trim())?.try_into().ok()
}

impl Install {
    /// The installation the binary at `exe` belongs to.
    ///
    /// # Errors
    ///
    /// When zirc's installer didn't put it there, this account can't write there, or its
    /// releases aren't on an `https://` URL.
    pub fn find(exe: &Path) -> Result<Self, Error> {
        Self::find_trusting(exe, Trust::from_env()?)
    }

    /// The installation the binary at `exe` belongs to, updating as `trust` says.
    fn find_trusting(exe: &Path, trust: Trust) -> Result<Self, Error> {
        let dir = exe
            .parent()
            .ok_or_else(|| Error::NotInstalled(exe.to_owned()))?
            .to_owned();
        let receipt = dir.join(RECEIPT);
        let text = match std::fs::read_to_string(&receipt) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::NotInstalled(dir));
            }
            Err(err) => return Err(Error::Receipt(receipt, err.to_string())),
        };
        let Receipt { releases } =
            toml::from_str(&text).map_err(|err| Error::Receipt(receipt, err.to_string()))?;
        if !trust.allows(&releases) {
            return Err(Error::Insecure(releases));
        }
        // Writable is what counts, whoever owns it: try it.
        let probe = dir.join(format!(".zirc-update-probe-{}", std::process::id()));
        if std::fs::write(&probe, "").is_err() {
            return Err(Error::NotWritable(dir));
        }
        let _ = std::fs::remove_file(probe);
        Ok(Self {
            dir,
            releases: releases.trim_end_matches('/').to_owned(),
            trust,
        })
    }

    /// Where the binaries are.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The latest release.
    ///
    /// # Errors
    ///
    /// When `latest.txt` can't be downloaded or names no version.
    pub async fn latest(&self) -> Result<Version, Error> {
        let url = format!("{}/latest.txt", self.releases);
        let text = fetch(&url, &self.trust).await?;
        String::from_utf8_lossy(&text).parse()
    }

    /// Replaces the binaries with `version`'s.
    ///
    /// # Errors
    ///
    /// When the checksum isn't signed with the release key, the archive can't be downloaded,
    /// doesn't match its checksum, lacks `zirc` or `zircd`, or the binaries can't be
    /// replaced. Binaries replaced before a failure stay replaced, each one whole.
    pub async fn install(&self, version: Version) -> Result<(), Error> {
        let name = format!("zirc-{version}-{}", target());
        let url = format!("{}/v{version}/{name}.tar.gz", self.releases);
        let checksum = format!("{name}.tar.gz.sha256");
        let published = fetch(&format!("{url}.sha256"), &self.trust).await?;
        let signature = fetch(&format!("{url}.sha256.sig"), &self.trust)
            .await
            .map_err(|err| {
                Error::Signature(checksum.clone(), format!("it has no signature ({err})"))
            })?;
        if !self
            .trust
            .verifies(&published, &String::from_utf8_lossy(&signature))
        {
            return Err(Error::Signature(
                checksum,
                "its signature isn't the release key's".into(),
            ));
        }
        // Signed for this archive, not another release's: its name has the version.
        let text = String::from_utf8_lossy(&published);
        let mut fields = text.split_whitespace();
        let expected = fields.next().unwrap_or_default().to_owned();
        if fields.next() != Some(&format!("{name}.tar.gz")) || fields.next().is_some() {
            return Err(Error::Signature(
                checksum,
                format!("it's signed, but not for {name}.tar.gz"),
            ));
        }
        let archive = fetch(&url, &self.trust).await?;
        let actual = sha256(&archive);
        if expected != actual {
            return Err(Error::Checksum(format!("{name}.tar.gz"), expected, actual));
        }

        // Unpacked beside the binaries, so each rename stays on one file system.
        let work = self
            .dir
            .join(format!(".zirc-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        let io = |what: &str| {
            let what = what.to_owned();
            move |err| Error::Io(what, err)
        };
        std::fs::create_dir(&work).map_err(io("making a directory to unpack in"))?;
        let replaced = self.replace(&work, &name, &archive).await;
        let _ = std::fs::remove_dir_all(&work);
        replaced
    }

    async fn replace(&self, work: &Path, name: &str, archive: &[u8]) -> Result<(), Error> {
        use std::os::unix::fs::PermissionsExt as _;
        let tarball = work.join(format!("{name}.tar.gz"));
        std::fs::write(&tarball, archive)
            .map_err(|err| Error::Io("saving the archive".into(), err))?;
        // The system's tar, as the installer uses.
        let output = tokio::process::Command::new("tar")
            .arg("-xzf")
            .arg(&tarball)
            .arg("-C")
            .arg(work)
            .output()
            .await
            .map_err(|err| Error::Io("running tar".into(), err))?;
        if !output.status.success() {
            return Err(Error::Release(format!(
                "couldn't unpack {name}.tar.gz: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let unpacked = work.join(name);
        for binary in BINARIES {
            let from = unpacked.join(binary);
            if !from.is_file() {
                return Err(Error::Release(format!("{name}.tar.gz has no {binary}")));
            }
            let to = self.dir.join(binary);
            let next = self
                .dir
                .join(format!(".{binary}.new-{}", std::process::id()));
            let io = |err| Error::Io(format!("replacing {}", to.display()), err);
            std::fs::copy(&from, &next).map_err(io)?;
            std::fs::set_permissions(&next, std::fs::Permissions::from_mode(0o755)).map_err(io)?;
            std::fs::rename(&next, &to).map_err(io)?;
        }
        for binary in RETIRED {
            let gone = self.dir.join(binary);
            match std::fs::remove_file(&gone) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                    return Err(Error::Io(format!("removing {}", gone.display()), err));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// What zircd last found when it checked for an update, in [`state`]: when it checked, and
/// a newer release it couldn't install, if it found one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// When it last checked, in seconds since the Unix epoch.
    pub checked: u64,
    /// A newer release it couldn't install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newer: Option<String>,
    /// Why it couldn't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

impl State {
    /// Checked just now, with nothing found to install.
    #[must_use]
    pub fn checked_now() -> Self {
        let checked = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        Self {
            checked,
            ..Self::default()
        }
    }

    /// What's in `path`, or nothing yet.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes it to `path`, whole.
    ///
    /// # Errors
    ///
    /// When it can't be written.
    ///
    /// # Panics
    ///
    /// Never: it serializes to JSON.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string(self).expect("the state serializes");
        crate::paths::write_private(path, &text)
    }

    /// One line for zirc's commands to print, while there's a newer release than this
    /// build that zircd couldn't install.
    #[must_use]
    pub fn notice(&self) -> Option<String> {
        let newer: Version = self.newer.as_deref()?.parse().ok()?;
        (newer > Version::current()).then(|| {
            format!(
                "{}, but zircd couldn't install it ({}): run `zirc update`",
                released(PROGRAM, newer),
                self.problem.as_deref().unwrap_or("it didn't say why")
            )
        })
    }
}

/// Where zircd keeps its [`State`].
#[must_use]
pub fn state(home: &Path) -> PathBuf {
    home.join("update.json")
}

/// How often zircd checks: a day, unless `ZIRC_UPDATE_EVERY` gives seconds, for tests.
#[must_use]
pub fn every() -> Duration {
    std::env::var("ZIRC_UPDATE_EVERY")
        .ok()
        .and_then(|seconds| seconds.parse().ok())
        .map_or(Duration::from_hours(24), Duration::from_secs)
}

/// `url`'s contents: over HTTPS, redirects included, or from a `file://` URL where `trust`
/// allows it, for tests.
async fn fetch(url: &str, trust: &Trust) -> Result<Vec<u8>, Error> {
    let failed = |err: &dyn fmt::Display| Error::Download(url.to_owned(), err.to_string());
    if !trust.allows(url) {
        return Err(Error::Insecure(url.to_owned()));
    }
    if let Some(path) = url.strip_prefix("file://") {
        return tokio::fs::read(path).await.map_err(|err| failed(&err));
    }
    // reqwest's rustls comes without a crypto provider: use ring, as the rest of zirc does.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .user_agent(concat!("zirc/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(300))
        .https_only(true)
        .build()
        .map_err(|err| failed(&err))?;
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|err| failed(&err))?;
    Ok(response.bytes().await.map_err(|err| failed(&err))?.to_vec())
}

/// `bytes`' SHA-256, in hex.
fn sha256(bytes: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

/// The bytes `text` gives in hex, if it's hex.
fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_by_number() {
        let v = |text: &str| text.parse::<Version>().unwrap();
        assert!(v("0.1.10") > v("0.1.9"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert_eq!(v("v0.1.11\n"), Version(0, 1, 11));
        assert_eq!(v("0.1.11").to_string(), "0.1.11");
        assert!("0.1".parse::<Version>().is_err());
        assert!("<html>".parse::<Version>().is_err());
    }

    #[test]
    fn hashes_as_sha256sum_does() {
        assert_eq!(
            sha256(b"zirc\n"),
            "1d54d3f17607800c36172b8e3a4949d2232c4b8630b825fbd9a9063d4eba02c5"
        );
    }

    #[test]
    fn names_platforms_as_releases_do() {
        let target = target();
        assert!(
            ["macos-arm64", "macos-x86_64", "linux-arm64", "linux-x86_64"]
                .contains(&target.as_str()),
            "{target}"
        );
    }

    /// A key of a test's own, trusted from `file://` URLs as [`TEST_KEY`] makes it, and its
    /// private half.
    fn test_trust() -> (Trust, String) {
        let (private, public) = generate().unwrap();
        let trust = Trust {
            key: public_key(&public).unwrap(),
            local: true,
        };
        (trust, private)
    }

    #[test]
    fn trusts_the_release_key_over_https_only() {
        let trust = Trust::from_env().unwrap();
        assert_eq!(trust.key(), RELEASE_KEY);
        assert!(trust.allows("https://storage.googleapis.com/zirc-releases"));
        assert!(!trust.allows("http://storage.googleapis.com/zirc-releases"));
        assert!(!trust.allows("file:///releases"));
        assert!(test_trust().0.allows("file:///releases"));
    }

    #[test]
    fn verifies_only_the_keys_own_signatures() {
        let (trust, private) = test_trust();
        let (signature, public) = sign(&private, b"checksum").unwrap();
        assert_eq!(public, trust.key());
        assert!(trust.verifies(b"checksum", &signature));
        assert!(trust.verifies(b"checksum", &format!("{signature}\n")));
        assert!(!trust.verifies(b"checksun", &signature));
        assert!(!trust.verifies(b"checksum", ""));
        assert!(!trust.verifies(b"checksum", "+0"));
        let (other, _) = test_trust();
        assert!(!other.verifies(b"checksum", &signature));
        assert!(sign("not a key", b"checksum").is_err());
    }

    #[test]
    fn reads_only_hex() {
        assert_eq!(unhex("0aFf"), Some(vec![10, 255]));
        assert_eq!(unhex("+a"), None);
        assert_eq!(unhex("abc"), None);
    }

    #[test]
    fn only_updates_what_the_installer_put_somewhere_writable() {
        let (trust, _) = test_trust();
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("zirc");
        let find = || Install::find_trusting(&exe, trust.clone());
        assert!(matches!(find(), Err(Error::NotInstalled(_))));
        std::fs::write(dir.path().join(RECEIPT), "releases = \"file:///r/\"\n").unwrap();
        let install = find().unwrap();
        assert_eq!(install.dir(), dir.path());
        assert_eq!(install.releases, "file:///r");
        std::fs::write(dir.path().join(RECEIPT), "where = 1\n").unwrap();
        assert!(matches!(find(), Err(Error::Receipt(..))));
        // Not over HTTPS: refused, with what to do instead. And file:// without a test key.
        std::fs::write(dir.path().join(RECEIPT), "releases = \"http://r/\"\n").unwrap();
        let err = find().unwrap_err();
        assert!(matches!(err, Error::Insecure(_)), "{err}");
        assert!(err.to_string().contains("https://"), "{err}");
        std::fs::write(dir.path().join(RECEIPT), "releases = \"file:///r/\"\n").unwrap();
        let release = Trust::from_env().unwrap();
        assert!(matches!(
            Install::find_trusting(&exe, release),
            Err(Error::Insecure(_))
        ));
    }

    #[test]
    fn notices_only_a_release_newer_than_this_one() {
        let mut state = State {
            checked: 1,
            newer: Some("999.0.0".into()),
            problem: Some("no space left".into()),
        };
        assert_eq!(
            state.notice().unwrap(),
            "zirc 999.0.0 is out, but zircd couldn't install it (no space left): run `zirc update`"
        );
        state.newer = Some(Version::current().to_string());
        assert_eq!(state.notice(), None);
        assert_eq!(State::default().notice(), None);
    }

    /// Releases with 9.9.9 latest, its archive staged but not yet published, and binaries
    /// installed from them, trusting a test key.
    struct Releases {
        _store: tempfile::TempDir,
        bin: tempfile::TempDir,
        install: Install,
        private: String,
        sum: PathBuf,
        /// The archive's checksum, as `sha256sum` writes it.
        good: String,
    }

    impl Releases {
        fn new() -> Self {
            let (trust, private) = test_trust();
            let releases = tempfile::tempdir().unwrap();
            let bin = tempfile::tempdir().unwrap();
            let url = format!("file://{}", releases.path().display());
            std::fs::write(bin.path().join(RECEIPT), format!("releases = \"{url}\"\n")).unwrap();
            for binary in ["zirc", "zircd", "zirc-tui"] {
                std::fs::write(bin.path().join(binary), "old").unwrap();
            }
            let name = format!("zirc-9.9.9-{}", target());
            let staging = releases.path().join("staging");
            std::fs::create_dir_all(staging.join(&name)).unwrap();
            for binary in BINARIES {
                std::fs::write(staging.join(&name).join(binary), format!("new {binary}")).unwrap();
            }
            let dir = releases.path().join("v9.9.9");
            std::fs::create_dir(&dir).unwrap();
            let archive = dir.join(format!("{name}.tar.gz"));
            let tar = std::process::Command::new("tar")
                .arg("-czf")
                .arg(&archive)
                .arg("-C")
                .arg(&staging)
                .arg(&name)
                .status()
                .unwrap();
            assert!(tar.success());
            std::fs::write(releases.path().join("latest.txt"), "9.9.9\n").unwrap();
            let good = format!(
                "{}  {name}.tar.gz\n",
                sha256(&std::fs::read(&archive).unwrap())
            );
            Self {
                install: Install::find_trusting(&bin.path().join("zirc"), trust).unwrap(),
                _store: releases,
                bin,
                private,
                sum: dir.join(format!("{name}.tar.gz.sha256")),
                good,
            }
        }

        /// Publishes `checksum`, signed with `key`.
        fn publish(&self, checksum: &str, key: &str) {
            std::fs::write(&self.sum, checksum).unwrap();
            let signature = sign(key, checksum.as_bytes()).unwrap().0;
            std::fs::write(self.sum.with_extension("sha256.sig"), signature).unwrap();
        }

        /// Installs 9.9.9, which is refused, saying `why`, and changes nothing.
        async fn refused(&self, why: &str) -> Error {
            let latest = self.install.latest().await.unwrap();
            assert_eq!(latest, Version(9, 9, 9));
            let err = self.install.install(latest).await.unwrap_err();
            assert!(err.to_string().contains(why), "{err}");
            let zirc = std::fs::read_to_string(self.bin.path().join("zirc")).unwrap();
            assert_eq!(zirc, "old");
            err
        }
    }

    #[tokio::test]
    async fn replaces_the_binaries_with_a_signed_release() {
        let releases = Releases::new();
        releases.publish(&releases.good, &releases.private);
        releases.install.install(Version(9, 9, 9)).await.unwrap();
        for binary in BINARIES {
            assert_eq!(
                std::fs::read_to_string(releases.bin.path().join(binary)).unwrap(),
                format!("new {binary}")
            );
        }
        // Nothing left behind but the binaries and the receipt: the retired zirc-tui is gone.
        let mut left: Vec<String> = std::fs::read_dir(releases.bin.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, [RECEIPT, "zirc", "zirc-remote", "zircd"]);
    }

    #[tokio::test]
    async fn refuses_a_release_that_doesnt_match_its_checksum() {
        let releases = Releases::new();
        let name = releases.good.split_whitespace().nth(1).unwrap().to_owned();
        releases.publish(&format!("{}  {name}\n", "0".repeat(64)), &releases.private);
        let err = releases.refused("doesn't match its SHA-256").await;
        assert!(matches!(err, Error::Checksum(..)), "{err}");
    }

    #[tokio::test]
    async fn refuses_a_release_without_the_release_keys_signature() {
        let releases = Releases::new();
        let good = releases.good.clone();
        // No signature.
        std::fs::write(&releases.sum, &good).unwrap();
        let err = releases.refused("has no signature").await;
        assert!(matches!(err, Error::Signature(..)), "{err}");
        // Another key's.
        releases.publish(&good, &test_trust().1);
        releases.refused("isn't the release key's").await;
        // The release key's, but for another release's archive.
        releases.publish(&good.replace("9.9.9", "9.9.8"), &releases.private);
        releases.refused("not for").await;
        // The release key's, of a checksum changed since.
        releases.publish(&good, &releases.private);
        std::fs::write(&releases.sum, good.replace('\n', " \n")).unwrap();
        releases.refused("isn't the release key's").await;
    }

    #[tokio::test]
    async fn downloads_nothing_but_https() {
        let err = fetch("http://example.com/latest.txt", &Trust::from_env().unwrap())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Insecure(_)), "{err}");
    }
}
