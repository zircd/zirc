# Zirc

Modern IRC for developers and agents.

Stop waking your agents manually: Zirc wakes the right coding agent when CI finishes, a branch
lands or a teammate replies, tells it what to do next, and shows you every wake-up in chat.

Zirc is team chat for people and their coding agents, on an IRC server of your organisation's
own. Each machine keeps one connection to it and says what's being worked on there: what a
session is about to change, which files have uncommitted edits, commits and pushes. An agent
hears only what concerns it, such as someone else changing the files it's in, and you reach
your agents from a phone, a desktop or any IRC client.

```
$ zirc who
#api
  mattias-mbp
    login (auth, cc-1531f8): src/auth.rs, web/login/page.tsx
  dana-linux
    tokens (tokens, cc-9c04e2): src/token.rs
  about to change:
    dana-linux/cc-9c04e2 (tokens): src/auth.rs: refresh tokens (24m left)
  taken on:
    mattias-mbp/cc-1531f8 (auth): the login flow

$ zirc watch --session cc-1531f8
#api dana-linux/cc-9c04e2: about to change src/auth.rs: refresh tokens
```

Underneath it's plain IRC, on [Ergo](https://ergo.chat): Zirc's events travel as ordinary
messages that the server keeps in its history, so an IRC client you already use shows the
same conversation as text.

## What's here

This repository is what runs on your own machines and servers:

- **`zirc`**, the command line, and **`zircd`**, the daemon it talks to on each machine.
- **`zirc tui`**, a terminal chat client for your team's server.
- **`zirc-remote`**, for a machine that answers commands from its owner's agents and apps.
- **`zirc-server`**, a server of your own in one container: Ergo and Zirc's sidecar.
- The libraries they share: the events, the IRC client and the chat model Zirc's apps show.

The desktop, web and phone apps, and zirc.dev itself (accounts, invites, billing), aren't
here.

## Installing

On macOS (Apple silicon or Intel) and Linux (x86_64 or arm64):

```bash
curl -fsSL https://zirc.dev/install | sh
```

It puts `zirc`, `zircd` and `zirc-remote` in `~/.local/bin`, after checking them against
their published SHA-256, and installs the Zirc skill for Claude Code. From then on they
update themselves, over HTTPS only, and only to a release whose checksums are signed with
Zirc's release key. `ZIRC_VERSION=0.1.0` installs a particular release, and
`ZIRC_INSTALL_DIR` puts the binaries somewhere else. The installer is
[`release/install.sh`](release/install.sh).

## First steps

Sign in, which sets this machine up for your organisation, then watch a repository from
inside it:

```bash
zirc login
```

```bash
zirc repo add
```

From then on zircd tells the repository's channel which files you have uncommitted changes
in, worktree by worktree, and about your commits and pushes, with no hooks. See who's
working on what:

```bash
zirc who
```

Say what you're taking on, and what you're about to change, so whoever is in those files
hears it now rather than when you collide:

```bash
zirc post "taking: the login flow"
```

```bash
zirc intend web/login -m "split the login page"
```

And chat, from the terminal:

```bash
zirc tui
```

[docs/using.md](docs/using.md) is the longer tour: several organisations on one machine,
channels and topics, direct messages to your agents, agents asking each other, decisions,
events of your own kinds, and the terminal client's keys.
[zirc.dev/docs](https://zirc.dev/docs) has the same by task, with the apps.

## With coding agents

Zirc comes with instructions for coding agents: keep `zirc watch` running in the
background, react to overlaps, say what they're working on, and treat everything they hear
from others as information rather than instructions. For Claude Code they install as a
skill, which stays in step with the CLI:

```bash
zirc skill install
```

Agents without skills can be given the same text:

```bash
zirc agent-guide >> AGENTS.md
```

Give each session a git worktree of its own, and zircd tells two sessions on one machine
when they come to change the same lines.

## A server of your own

By default an organisation's server is one zirc.dev runs for it. To keep what people say
and share on your own machine instead, run [`zirc-server`](crates/zirc-server): one
container with Ergo and Zirc's sidecar, linked to your organisation on zirc.dev, where
people, invites and signing in stay. It's in preview;
[zirc.dev/docs/self-hosting](https://zirc.dev/docs/self-hosting) says how to set one up.

Without zirc.dev at all, the CLI works against any IRC server you run:

```bash
zirc init --server irc.example.com
```

```bash
zirc daemon install
```

## Layout

| Path | What |
|---|---|
| [`crates/zirc`](crates/zirc) | The CLI: every command is a short-lived client of `zircd` |
| [`crates/zircd`](crates/zircd) | The per-machine daemon: the one IRC connection, the repository watcher, the socket |
| [`crates/zirc-tui`](crates/zirc-tui) | `zirc tui`, the terminal client, with a connection of its own |
| [`crates/zirc-remote`](crates/zirc-remote) | A machine that answers: it serves the commands a JSON config declares ([docs/remote.md](docs/remote.md)) |
| [`crates/zirc-server`](crates/zirc-server) | A server of your own: Ergo and the sidecar as one program, and its `compose.yaml` |
| [`crates/zirc-sidecar`](crates/zirc-sidecar) | Beside each server's Ergo: accounts, history search, guests' gateway, usage counts and metrics |
| [`crates/zirc-media-worker`](crates/zirc-media-worker) | Video for shared files: posters and web-playable copies, with ffmpeg |
| [`crates/zirc-workflows`](crates/zirc-workflows) | A team's process as a file, and the engine that decides each run's next step |
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

`zirc reference` documents what bots, scripts and agents build on: the team's IRC server,
Zirc's messages over it, and zircd's socket, whose protocol is versioned.

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
