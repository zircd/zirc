//! This machine's processes, as `ps` says them: which one a `zirc` command was run from (its
//! agent, or a person's terminal), and whether anything heavy runs under it, such as a
//! Bazel build. zircd gives back a lease whose holder has run nothing for a while.

use std::collections::BTreeMap;
use tokio::process::Command;

/// What runs a command for an agent or a person without being them: shells and wrappers,
/// passed over when looking for whom a command was run by.
const WRAPPERS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "dash",
    "fish",
    "env",
    "nice",
    "nohup",
    "timeout",
    "sandbox-exec",
    "zirc",
];

/// Heavy work a lease is held for, by the start of a process's name: builds, tests and
/// devices.
const HEAVY: &[&str] = &[
    "bazel",
    "cargo",
    "rustc",
    "xcodebuild",
    "xcrun",
    "simctl",
    "swift",
    "clang",
    "gradle",
    "java",
    "emulator",
    "qemu",
    "adb",
    "make",
    "ninja",
];

/// Every process, by id: its parent and its name.
#[derive(Debug, Default)]
pub struct Procs {
    procs: BTreeMap<u32, (u32, String)>,
}

impl Procs {
    /// This machine's processes, from `ps`, run without blocking the daemon: on a busy
    /// machine it can take a while. `None` when it can't say.
    pub async fn read() -> Option<Self> {
        let out = Command::new("ps")
            .args(["-axo", "pid=,ppid=,comm="])
            .kill_on_drop(true)
            .output()
            .await
            .ok()?;
        out.status
            .success()
            .then(|| Self::parse(&String::from_utf8_lossy(&out.stdout)))
    }

    /// Whether process `pid` runs: from `/proc` where there is one, as on Linux, whose
    /// smallest images have no `ps`; from `ps` elsewhere, such as macOS. `None` when it
    /// can't say.
    pub async fn alive(pid: u32) -> Option<bool> {
        if std::path::Path::new("/proc/self").exists() {
            return Some(std::path::Path::new("/proc").join(pid.to_string()).exists());
        }
        Self::read().await.map(|procs| procs.has(pid))
    }

    /// `ps -o pid=,ppid=,comm=`'s lines. A name is the last part of its path: macOS gives
    /// the whole path.
    pub fn parse(text: &str) -> Self {
        let procs = text
            .lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                let pid = words.next()?.parse().ok()?;
                let parent = words.next()?.parse().ok()?;
                let command = words.collect::<Vec<_>>().join(" ");
                let name = command.rsplit('/').next().unwrap_or_default().to_owned();
                Some((pid, (parent, name)))
            })
            .collect();
        Self { procs }
    }

    /// Whether `pid` is running.
    pub fn has(&self, pid: u32) -> bool {
        self.procs.contains_key(&pid)
    }

    /// Whom `pid` was run by: its nearest forebear that's not a shell or wrapper, such as an
    /// agent. `None` when that's the system's first process, or isn't known.
    pub fn runner_of(&self, pid: u32) -> Option<u32> {
        let mut at = pid;
        for _ in 0..64 {
            let (parent, name) = self.procs.get(&at)?;
            if !WRAPPERS.contains(&name.as_str()) {
                return (at > 1).then_some(at);
            }
            at = *parent;
        }
        None
    }

    /// The name of something heavy running under `pid`, if anything is.
    pub fn heavy_under(&self, pid: u32) -> Option<String> {
        let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for (&child, (parent, _)) in &self.procs {
            children.entry(*parent).or_default().push(child);
        }
        let mut stack = children.get(&pid).cloned().unwrap_or_default();
        while let Some(at) = stack.pop() {
            let name = &self.procs[&at].1;
            if HEAVY.iter().any(|heavy| name.starts_with(heavy)) {
                return Some(name.clone());
            }
            stack.extend(children.get(&at).into_iter().flatten());
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = "    1     0 /sbin/launchd
  500     1 /Applications/Claude.app/Contents/MacOS/claude
  501   500 /bin/zsh
  502   501 /Users/me/.local/bin/zirc
  503   500 node
  504   500 /bin/zsh
  505   504 /Users/me/Library/Caches/bazelisk/downloads/abc/bin/bazel
  600     1 /bin/zsh
  601   600 zirc
";

    #[test]
    fn finds_whom_a_command_was_run_by_past_shells() {
        let procs = Procs::parse(PS);
        assert_eq!(procs.runner_of(502), Some(500));
        assert_eq!(procs.runner_of(500), Some(500));
        // Run from a shell launchd started: nobody to watch.
        assert_eq!(procs.runner_of(601), None);
        assert_eq!(procs.runner_of(999), None);
        assert!(procs.has(500) && !procs.has(999));
    }

    #[test]
    fn says_whats_heavy_under_a_runner() {
        let procs = Procs::parse(PS);
        assert_eq!(procs.heavy_under(500).as_deref(), Some("bazel"));
        // An MCP server or the watch is nothing heavy.
        let idle = Procs::parse(&PS.replace("504   500", "504     1"));
        assert_eq!(idle.heavy_under(500), None);
    }
}
