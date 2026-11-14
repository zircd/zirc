//! The server's own secrets, made on its first start and kept in its data directory, readable
//! by its owner alone: what a hosted server keeps as Kubernetes secrets
//! (`zirc_sidecar::secrets`).

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use zirc_sidecar::environment::Secret;

/// The file they're kept in, in the data directory.
pub const FILE: &str = "secrets.json";

/// A password and the bcrypt hash Ergo reads.
#[derive(Serialize, Deserialize, Clone)]
pub struct Password {
    /// The password itself.
    pub password: String,
    /// Its bcrypt hash.
    pub hash: String,
}

impl Password {
    fn new() -> Self {
        let (password, hash) = zirc_sidecar::secrets::password();
        Self { password, hash }
    }
}

/// The server's secrets.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Secrets {
    /// The server password, which accounts' logins stand in for.
    pub gate: Password,
    /// The operator's, which the sidecar logs in to Ergo with.
    pub oper: Password,
    /// Ergo's API's, which the sidecar presents.
    pub api_token: String,
    /// The account endpoint's, which zirc.dev presents: given by it as the server joins.
    pub accounts_token: String,
    /// The one that sets teams, which zirc.dev's tenant controller presents: given by it as
    /// the server joins. Made when it's missing, for a server whose secrets predate it.
    #[serde(default = "zirc_sidecar::secrets::token")]
    pub controller_token: String,
}

impl Secrets {
    /// The secrets in `data`, made and written there first if there are none.
    ///
    /// # Errors
    ///
    /// When they can't be read or written.
    pub fn read_or_make(data: &Path) -> anyhow::Result<Self> {
        let path = path(data);
        if path.exists() {
            return Self::read(data);
        }
        let secrets = Self {
            gate: Password::new(),
            oper: Password::new(),
            api_token: zirc_sidecar::secrets::token(),
            accounts_token: zirc_sidecar::secrets::token(),
            controller_token: zirc_sidecar::secrets::token(),
        };
        secrets.write(data)?;
        tracing::info!("made the server's secrets in {}", path.display());
        Ok(secrets)
    }

    /// The secrets in `data`.
    ///
    /// # Errors
    ///
    /// When there are none, or they can't be read.
    pub fn read(data: &Path) -> anyhow::Result<Self> {
        let path = path(data);
        let text = fs::read(&path).with_context(|| {
            format!(
                "reading {}: has the server started in this data directory?",
                path.display()
            )
        })?;
        let secrets: Self = serde_json::from_slice(&text)?;
        // One made just now, by default, is kept, so it's the same the next time.
        let again: serde_json::Value = serde_json::from_slice(&text)?;
        if again.get("controllerToken").is_none() {
            write_private(&path, &serde_json::to_vec_pretty(&secrets)?)?;
        }
        Ok(secrets)
    }

    /// Writes them to `data`, readable by its owner alone.
    ///
    /// # Errors
    ///
    /// When they can't be written.
    pub fn write(&self, data: &Path) -> anyhow::Result<()> {
        Ok(write_private(
            &path(data),
            &serde_json::to_vec_pretty(self)?,
        )?)
    }

    /// The value Ergo reads for `secret`.
    #[must_use]
    pub fn for_ergo(&self, secret: Secret) -> String {
        match secret {
            Secret::GateHash => self.gate.hash.clone(),
            Secret::OperHash => self.oper.hash.clone(),
            Secret::ApiBearerTokens => zirc_sidecar::secrets::bearer_tokens(&self.api_token),
        }
    }
}

fn path(data: &Path) -> PathBuf {
    data.join(FILE)
}

/// Writes `bytes` to `path` readable by its owner alone, all at once: to a file beside it
/// first, then moved over it.
///
/// # Errors
///
/// When it can't be written.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let partial = path.with_extension("partial");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&partial)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(partial, path)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn they_re_made_once_and_kept_private() {
        let data = tempfile::tempdir().unwrap();
        let first = Secrets::read_or_make(data.path()).unwrap();
        let second = Secrets::read_or_make(data.path()).unwrap();
        assert_eq!(first.api_token, second.api_token);
        assert_eq!(first.gate.hash, second.gate.hash);
        let mode = fs::metadata(path(data.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn ergo_gets_hashes_never_passwords() {
        let data = tempfile::tempdir().unwrap();
        let secrets = Secrets::read_or_make(data.path()).unwrap();
        assert!(secrets.for_ergo(Secret::GateHash).starts_with("$2a$"));
        assert!(
            secrets
                .for_ergo(Secret::ApiBearerTokens)
                .contains(&secrets.api_token)
        );
    }
}
