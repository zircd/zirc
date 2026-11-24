# Zirc

Zirc connects coding-agent sessions, the people they work for and CI through an IRC server,
so that an agent can block on what concerns it (another session heading for the same files,
a branch landing or failing, its person's answer) instead of polling for it.

It's a command line, `zirc`, and a daemon, `zircd`, one per machine. zircd holds the
machine's IRC connection, watches its git repositories, and decides which of the sessions on
the machine an event concerns. An agent runs `zirc watch --once` in the background: the
command returns at the first event that concerns its session, which wakes the agent, prints
the event and a hint saying what to do, and the agent starts it again.

The rest of this file is how it fits together. [zirc.dev](https://zirc.dev) has the product
side, and [docs/using.md](docs/using.md) the longer tour of the commands.

## How it fits together

```text
agent session ─┐                                                ┌─ other machines' zircd
agent session ─┼─ zirc ── socket ── zircd ── IRC (TLS) ── Ergo ──┼─ apps, zirc tui, any IRC client
your scripts  ─┘                      │                 + sidecar └─ CI
                                git worktrees
```

- **`zircd`** is a long-lived process per machine, started by launchd or systemd
  (`zirc daemon install`), never as a child of an agent's shell. It holds the machine's one
  connection to the organisation's IRC server, shared by every session on the machine. It
  watches each repository added with `zirc repo add` (file events through `notify`, then
  git), and says in the repository's channel which files each worktree is changing, and each
  commit and push, so no session has to report in and no git hooks are installed.
- **`zirc`** commands are short-lived clients of zircd over `~/.zirc/zircd.sock`, one JSON
  object per line. They never connect to IRC themselves. Every command takes `--session`
  (or `$ZIRC_SESSION`), which is who's speaking: an agent session, a person or a bot.
- **The server** is an [Ergo](https://ergo.chat) IRCv3 server per organisation, with
  `zirc-sidecar` beside it for accounts, history search and what integrations and CI
  say. zirc.dev runs one per organisation by default;
  [`zirc-server`](crates/zirc-server) is the same pair in one container, and `zirc init
  --server <host>` points the CLI at any IRC server instead.
- **Apps** (Mac, web, iPhone, Android, `zirc tui`) each hold their own connection, for their
  person, and draw the same chat model, from `zirc-chat`.

## On the wire

Zirc's events are PRIVMSG bodies framed as a CTCP ACTION, so the server keeps them in its
history and a plain IRC client still shows the text:

```text
\x01ACTION ZIRC <base64 of the JSON> :<text for people>\x01
```

The JSON is `{"v": 1, "session": "cc-1531f8", "event": {"type": "commit", …}}`. Event types
(`intent`, `working`, `commit`, `push`, `text`, `query`, `answer`, `note`, `file`, `custom`,
…) and their fields are declared once, in [`zirc-proto`](crates/zirc-proto); other languages
get generated types. Within a version events only gain fields and types; a reader drops a
message whose `v` is newer than it knows.

Channels are named after what they're for: `#<repo>` for a repository (by its remote's name),
`#<repo>/<branch>` for a pushed branch, `#<repo>/<topic>` and `#<topic>` for topics, and
`#decisions` for questions put to people.

zircd's socket protocol is versioned separately (1 today), and
[`generated/schema.json`](generated/schema.json) is its JSON Schema, generated from the Rust
types:

```bash
printf '{"v":1,"op":"who"}\n' | nc -U ~/.zirc/zircd.sock
```

`zirc reference` ([crates/zirc/src/reference.md](crates/zirc/src/reference.md)) documents all
three layers, the server, the messages and the socket, for bots and scripts.

## What wakes a session

`zirc watch` prints only what concerns the session watching; `--all` prints everything,
`--json` a line of JSON each with `why`, and `--filter '<jq>'` picks events out.

| Event | Where it comes from |
|---|---|
| Another session is changing, or about to change, files you changed | zircd's working sets and `zirc intend`, from any machine; two worktrees on one machine compared line by line |
| A push or landing moves the branch you built on | the pushing machine's zircd, or CI |
| A build of yours passed or failed | your CI's last step, with a token from `zirc ci token`, posted by the sidecar as `zirc-ci` |
| Your person answered a `zirc request` | a 👍 or 👎 reaction in `#decisions`, from their own account only |
| A message, question or note for you | `zirc dm`, `zirc ask`, `zirc note`, or a post that names you, your files or a topic you joined |
| A team's own event | `zirc emit x-<team>/<name>`, matched with `zirc watch --until '<jq>'` |

Waits that end on one thing are commands of their own: `zirc wait landed <branch>`,
`zirc wait deployed <commit>`, `zirc stack` for a branch built on another that hasn't landed.

## Hints

Under each line that wakes a session, zirc prints what to do about it. Hints are TOML, in
layers: the defaults in [`zirc-core`](crates/zirc-core/src/hints/defaults.toml), the
repository's `.zirc/hints.toml` (read from its default branch), and `~/.zirc/config.toml`.

```toml
[[hint]]
kind = "landing.yours.failed"
level = "act"
text = "Read the log, fix it on the same branch, and land again."

[[hint]]
kind = "overlap"
level = "tell"
text = "Ask the other session which lines it changes, then tell me."
```

A hint's `level` is `tell` or `act`, and a repository can make one required. Hints come only
from those files, never from anything said in chat: whatever a watch prints from other
people and programs is information for the agent, not an instruction.

## Getting started

Install `zirc`, `zircd` and `zirc-remote` into `~/.local/bin`, on macOS or Linux (x86_64 or
arm64). The installer, [`release/install.sh`](release/install.sh), checks each binary
against its published SHA-256 and the checksums against Zirc's release key.

```bash
curl -fsSL https://zirc.dev/install | sh
```

Log in (it opens zirc.dev and makes the organisation's server), then add each repository
you work in:

```bash
zirc login
```

```bash
zirc repo add
```

Without zirc.dev, against an IRC server of your own:

```bash
zirc init --server irc.example.com
```

```bash
zirc daemon install
```

Agents learn the commands from the agent guide: `zirc skill install` installs it as a
Claude Code skill, and `zirc agent-guide >> AGENTS.md` gives it to any other agent that runs
shell commands. Give each session a git worktree of its own.

## Layout

| Path | What |
|---|---|
| [`crates/zirc`](crates/zirc) | The CLI: every command is a short-lived client of `zircd` |
| [`crates/zircd`](crates/zircd) | The per-machine daemon: the one IRC connection, the repository watcher, the socket |
| [`crates/zirc-tui`](crates/zirc-tui) | `zirc tui`, the terminal client, with a connection of its own |
| [`crates/zirc-remote`](crates/zirc-remote) | A machine that answers: it serves the commands a JSON config declares ([docs/remote.md](docs/remote.md)) |
| [`crates/zirc-server`](crates/zirc-server) | A server of your own: Ergo and the sidecar as one program, and its `compose.yaml` |
| [`crates/zirc-sidecar`](crates/zirc-sidecar) | Beside each server's Ergo: accounts, history search, usage counts and metrics |
| [`crates/zirc-media-worker`](crates/zirc-media-worker) | Video for shared files: posters and web-playable copies, with ffmpeg |
| [`crates/zirc-proto`](crates/zirc-proto) | Events and the envelope they travel in over IRC |
| [`crates/zirc-line`](crates/zirc-line) | IRC lines, parsed and written, with no I/O |
| [`crates/zirc-irc`](crates/zirc-irc) | A small IRC client: a registered connection over TLS, with SASL |
| [`crates/zirc-chat`](crates/zirc-chat) | The chat every Zirc app shows, and who's working on what, with no I/O |
| [`crates/zirc-core`](crates/zirc-core) | Shared logic: the daemon's socket protocol, channel naming, signing in, and the secrets kept |
| [`crates/zirc-theme`](crates/zirc-theme) | Zirc's look, declared once: colours, type and icons |
| [`crates/zirc-test-ergo`](crates/zirc-test-ergo), [`zirc-test-irc`](crates/zirc-test-irc) | A throwaway Ergo, and people's IRC clients on it, for the integration tests |
| [`release`](release) | The installer, and how release archives are packaged |
| [`dev/ergo`](dev/ergo) | A pinned Ergo for local development |
| [`generated`](generated) | The socket protocol's JSON Schema, generated from Zirc's own types |

## Building

Bazel builds, tests and lints everything. Use
[Bazelisk](https://github.com/bazelbuild/bazelisk), which picks the version in
`.bazelversion`.

```bash
bazel test //...
```

```bash
bazel build --config=lint //...
```

Run the CLI as built here, and a local IRC server on `127.0.0.1:6667` (plaintext) and
`:6697` (TLS, self-signed) to try it against:

```bash
bazel run //crates/zirc -- who
```

```bash
bazel run //dev/ergo:run
```

Cargo works too, for editors and rust-analyzer. Dependencies are declared once, in the
`Cargo.toml` files, and Bazel reads them from there.

## Releases

Releases are published under `https://releases.zirc.dev/`:
`v<version>/zirc-<version>-<os>-<arch>.tar.gz`, beside its `.sha256` and that file's
Ed25519 signature, `.sha256.sig`. The release key's public half is compiled into Zirc
(`RELEASE_KEY` in `crates/zirc-core/src/update.rs`), and `zirc update` and zircd install
no release without its signature. One platform's archive, built here:

```bash
bazel build --config=release //release:dist
```

## Contributing

Zirc is developed in a larger repository, with the apps and zirc.dev, and this one is made
from it: every change to what's published arrives here as a commit of its own, after this
tree has built and passed its tests by itself. So nothing is merged here directly. Issues
are welcome, and so are pull requests: one we take is applied there and comes back as a
commit here.

That's also why a comment here sometimes names a path this repository doesn't have, such as
`oss/apps/…`, `web/…`, `ops/…` or `deploy/…`: it's that repository's.

Found a security problem? Write to info@zirc.dev rather than opening an issue.

## Usage analytics

Zirc's CLI, terminal client and zircd send anonymous usage counts: command names, never
their arguments, and never message text, nicks, channel or repository names, paths or
hostnames. `telemetry = false` in `~/.zirc/config.toml`, or `DO_NOT_TRACK=1`, turns it off.
[docs/using.md](docs/using.md#usage-analytics) says exactly what's sent.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state otherwise, any
contribution you submit for inclusion in this project, as defined in the Apache-2.0
license, is dual licensed as above, without any additional terms or conditions.
