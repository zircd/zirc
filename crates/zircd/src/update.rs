//! zircd updating itself: about once a day it checks for a newer release and, where zirc's
//! installer put it, installs it, then asks the daemon to restart from the new binary. See
//! [`zirc_core::update`].

use std::path::{Path, PathBuf};
use std::time::Duration;
use zirc_proto::PROGRAM;

use tokio::sync::mpsc;
use zirc_core::telemetry;
use zirc_core::update::{self, Install, State, Version};
use zirc_proto::telemetry::{Event, UpdateInstalled, UpdateOffered};

use crate::router::Call;

/// Checks for a newer release every [`update::every`], installing it when there is one.
/// Returns at once for binaries the installer didn't put somewhere writable: those don't
/// update themselves.
pub async fn check_daily(home: PathBuf, inputs: mpsc::UnboundedSender<Call>) {
    let install = match std::env::current_exe()
        .map_err(|err| err.to_string())
        .and_then(|exe| Install::find(&exe).map_err(|err| err.to_string()))
    {
        Ok(install) => install,
        Err(why) => {
            tracing::debug!("not updating: {why}");
            return;
        }
    };
    let every = update::every();
    let path = update::state(&home);
    loop {
        // When it last checked, as this zircd or one before it: a restart doesn't check
        // again.
        let state = State::load(&path);
        let due = state.checked + every.as_secs();
        let now = State::checked_now().checked;
        tokio::time::sleep(Duration::from_secs(due.saturating_sub(now))).await;
        if let Some(version) = check(&install, &path).await {
            let _ = inputs.send(Call::Updated(version));
            return;
        }
    }
}

/// Checks once, saying what it found in `path`: the release it installed, if it did.
async fn check(install: &Install, path: &Path) -> Option<Version> {
    let mut state = State::checked_now();
    let installed = match install.latest().await {
        // Not worth a notice: the next check will likely get through.
        Err(err) => {
            tracing::warn!("checking for an update: {err}");
            None
        }
        Ok(latest) if latest <= Version::current() => {
            tracing::debug!("{PROGRAM} {} is up to date", Version::current());
            None
        }
        Ok(latest) => {
            tracing::info!(
                "installing {PROGRAM} {latest} in {}",
                install.dir().display()
            );
            telemetry::record(&Event::UpdateOffered(UpdateOffered {
                to: latest.to_string(),
            }));
            match install.install(latest).await {
                Ok(()) => {
                    telemetry::record(&Event::UpdateInstalled(UpdateInstalled {
                        from: Version::current().to_string(),
                        to: latest.to_string(),
                    }));
                    Some(latest)
                }
                Err(err) => {
                    tracing::warn!("couldn't install {PROGRAM} {latest}: {err}");
                    state.newer = Some(latest.to_string());
                    state.problem = Some(err.to_string());
                    None
                }
            }
        }
    };
    if let Err(err) = state.save(path) {
        tracing::warn!("saving {}: {err}", path.display());
    }
    installed
}
