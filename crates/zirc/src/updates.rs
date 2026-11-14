//! `zirc update`, and saying when a newer release is waiting.

use std::io::Write;
use std::path::Path;

use anyhow::Result;
use zirc_core::update::{self, Install, Version};
use zirc_proto::PROGRAM;

use crate::client::shown;
use crate::daemon::restart;

/// Says a newer release zircd found but couldn't install, if there's one: on stderr, so
/// it's never in what a script reads.
pub(crate) fn say_update(home: &Path) {
    if let Some(notice) = update::State::load(&update::state(home)).notice() {
        eprintln!("{PROGRAM}: {notice}");
    }
}

/// `zirc update`: the latest release in place of this one, and zircd restarted to run it.
pub(crate) async fn update(home: &Path, check: bool, out: &mut impl Write) -> Result<()> {
    let current = Version::current();
    let install = Install::find(&std::env::current_exe()?)?;
    let latest = install.latest().await?;
    if latest <= current {
        writeln!(out, "{PROGRAM} {current} is up to date")?;
        return Ok(());
    }
    if check {
        writeln!(
            out,
            "{} (this is {current}): run `zirc update` to install it",
            zirc_core::update::released(zirc_proto::PROGRAM, latest)
        )?;
        return Ok(());
    }
    install.install(latest).await?;
    // What zircd last found is out of date now, any notice of it included.
    let _ = update::State::checked_now().save(&update::state(home));
    writeln!(
        out,
        "updated {PROGRAM} {current} → {latest} in {}",
        shown(install.dir())
    )?;

    if !restart(home, out).await? {
        writeln!(out, "restart zircd to run {latest}: zirc daemon restart")?;
    }
    Ok(())
}
