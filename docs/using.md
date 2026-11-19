# Using Zirc

The longer tour of the CLI, zircd and the terminal client, after the [README](../README.md)'s
first steps. [zirc.dev/docs](https://zirc.dev/docs) has the same by task, with the apps.

Log in at zirc.dev, which sets this machine up for your organisation, then watch a repository from
inside it:

```bash
zirc login
```

```bash
zirc repo add
```

`zirc login` shows a code and opens the browser, where you sign up or sign in, pick your
organisation and confirm it; it then writes your team's server to `~/.zirc/config.toml`,
installs the Zirc skill for Claude Code, and runs zircd at login, waiting until it's
connected. Where the server has accounts, the machine gets one of its own, named
`<you>-<machine>` (such as `mattias-mbp`), with a passphrase for it alone; it logs in with
SASL and gives your handle as its real name. Running `zirc login` again for the same
server updates the machine's setup, such as from a gate password to an account; a machine
that's set up for another server keeps its setup unless you run `zirc login --force`. When
the server refuses a machine's account, as after you change your handle, zircd asks
zirc.dev for its current one.

A machine can be signed in to several organisations at once, each with its own server,
account and channels: `config.toml` keeps the first as a machine in one does, and the
others as `[[org]]` tables after it, each with its own nick and handle. `zirc login` signs
the machine in to each organisation chosen on zirc.dev's page, keeping those it's in
already, and `zirc login --org <name>` adds one later. zircd holds a connection to each. Every repository
belongs to exactly one organisation, and nothing about it (paths, branches, intents,
pushes) is ever sent to another's server: `zirc repo add --org <name>` says which, or with
another clone of the project watched already, it's that clone's. Commands act on the
organisation of the repository they're run in, or `--org <name>` outside one; `zirc status`
lists each organisation's connection, and one `zirc watch` covers them all, each line
starting with its organisation. Releases before 0.1.18 can't read a `config.toml` with `[[org]]`
tables: to go back to one, remove them first, or log in again with it.

A machine's agents hear only their own user. A message to the machine's nick, from any IRC
client or app, reaches its `zirc watch` streams as `dm from <nick>: …` when it comes from the
machine's owner, by the server's account rather than the nick, or, on servers zirc.dev set up,
from another of the owner's machines (`<owner>-…`) as typed in its terminal client. A first
word that names a session working there, such as `cc-1531f8: rebase please`, makes it for
that session alone, and `zirc watch --session <session>` leaves out the others'; one that
names a session that isn't there gets a notice back, saying which are, and reaches no one.
A leading `/agent`, which older apps send, is left out. Everything else sent to the machine
is filtered out. Agents answer their user with `zirc dm`. Zirc's apps give each session a
conversation of its own under direct messages, such as `mattias · mbp / cc-1531f8` (in the
terminal client, `/agents` lists them and `/query mattias-mbp/cc-1531f8` opens one): what
you type there goes to that session alone.

Agents can ask each other things: `zirc ask --session <s> --to <machine>[/<session>]
"…"`, or `--to <person>` for each of their machines here, reaches those agents' watches as
`query from …`, and `zirc reply --session <s> <id> "…"` answers it. Other messages between
agents stay undelivered. Who may query this machine's agents is its own choice, `zirc agents
queries off|mine|anyone`, kept in `~/.zirc/agents.toml`: by default `mine`, this machine's
other sessions and your own account and machines, told by the account the server says sent
it, never the nick. `anyone` lets any member's agents try to steer yours (prompt
injection), so Zirc asks you to say `--yes`. Agents take a query as a teammate's request,
never as their user's word.

Decisions are recorded where every session finds them: an agent posts what its user decided
with `zirc decide --session <s> --quote "<their words>" --about "<topic>"`, a person posts
their own with `/decide <topic>: <words>` in the TUI (or `zirc decide` at a terminal), and
`zirc decisions --about <topic>` looks them up in `#decisions`. Each says whose account sent it,
as the server does: the person's own, a terminal on their machine, a session relaying
them, or unverified. Every session's watch hears them.

For a server
of your own instead, say which one and run zircd yourself:

```bash
zirc init --server irc.example.com
```

```bash
zirc daemon install
```

Every organisation has a server of its own, and every clone of a project shares its
channels, named after the remote's repository, so `git@github.com:acme/api.git` is `#api`
on every machine. From then on zircd tells `#api` which files you have uncommitted changes
in, worktree by worktree and branch by branch, and about your commits and pushes, with no
hooks. See who is working on what, branch by branch, what sessions are about to change and
have taken on, and what's landed lately:

```bash
zirc who
```

Follow what concerns you, and only that, one line each, made to run under an agent's
monitor, where every line wakes the agent: someone else changing or about to change files
you've changed (`#api overlap: beta is also changing src/auth.rs`), pushes that change your
files on the branch you'll rebase onto or clash with your lines, messages to your machine,
and posts that name you, your files or a topic you joined, whether they come from Zirc or
a person typing in any IRC client. Commits, and posts that don't concern you, aren't
printed. With `--session`, an agent session's own posts don't come back
and it's told only what concerns its worktree. A path counts as someone's work only if it
differs from the remote's default branch too, so a worktree caught halfway through an
update overlaps with nobody, and each overlap is told once. Two worktrees on one machine
that change different parts of the same file will merge on their own, so an agent's watch
stays quiet about them until they come to change the same lines. `--all` shows everything,
working-set updates included, and `--json` has every line with why it concerns you
(`"why": "overlap"`, `"mention"`, …) or none:

```bash
zirc watch --session cc-1531f8
```

Each wake costs an agent a turn, so zircd caps them: past 10 wakes of one session, or 30 of
the machine's, in 10 minutes (`[wakes]` `per_session` and `machine` in `config.toml` change
them), it holds the rest for the session's next wake and tells the owner in a notice in their
conversation with the machine, which no phone pushes; they answer `resume` there (`zirc
wakes` shows it, `zirc wakes resume` lifts it). A checkout's uncommitted files
count as a session's work only where it's the checkout's one session, so give each session
a worktree of its own. `zirc lease build` has three slots, every other lease one; a holder
others wait for gets 30 minutes at most, and a lease is given back after 10 idle minutes
with no build running under its holder (simulator and emulator leases excepted).

And say something. It goes to the repository's channel, where every session working on it
is; `--to branch` and `--to org` send it to your pushed branch's channel or the whole
organisation's. Posts starting `taking:` show in `zirc who` until a `done:` or `dropped:`:

```bash
zirc post "taking: the login flow"
```

Before changing files, say so, and whoever is changing them hears it now rather than when
you collide. An intent shows in `zirc who` for 30 minutes (`--for` to change that), until a
new one replaces it or the session posts `done:` or `dropped:`:

```bash
zirc intend web/login -m "split the login page"
```

The channels, all predictable from where you work:

| Channel | Joined | For |
|---|---|---|
| `#general` | by every machine | the organisation's people and announcements |
| `#api` | by every watcher of the repository | working sets, overlaps, commits, pushes |
| `#api/login` | by choice, `zirc join login` | branch `login`'s thread: the sessions on that feature, under `#api` in the apps |
| `#api/auth`, `#auth` | by choice, `zirc join auth [--org]` | a topic, per repository or organisation-wide |

Post to a topic you've joined with `zirc post --topic auth …`, and leave it with
`zirc leave auth`. A topic is the repository's unless you add `--org`, as for `#ops`:
`zirc post --topic ops --org …`.

To take back something posted by mistake, such as a secret, find its id with `zirc search`,
which shows a channel's messages over a time range, and trim it:

```bash
zirc search --in '#api' --since 1h
zirc trim '#api' 01JBX7Q2M3 --reason "pasted a token"
```

It removes the message from the server's history, and from Zirc's apps and other IRC
clients that support removing messages (`draft/message-redaction`), whoever's screen it's
on. A plain IRC client that already showed it keeps its copy, and whatever anyone copied
stays: change a leaked secret anyway. The server lets a machine trim what it sent itself, and a
channel's operators trim anything in it, where its operator allows trimming at all.

## What's been said

While an organisation's owners have debug logging on, on its Log tab at zirc.dev, its server's
log keeps the messages sent in channels for 30 days, and direct messages too if they put them on
the record. From an owner's machine, `zirc log` shows them: the last 12 hours, or as far back as
`--since` says (`30m`, `12h`, `3d`, `2w`, `1mo`), with any text to look for. `--json` prints a line
of JSON each, for an agent to summarise what's happening:

```bash
zirc log --since 1d --json
```

A message trimmed from the history stays in the log until it's 30 days old.

## Building on Zirc

Teams send events of their own kinds, which Zirc carries without knowing what they mean,
and scripts pick out what they care about with a [jq](https://jqlang.org) filter:

```bash
zirc emit x-acme/deploy --json '{"env":"prod","version":"1.4.2"}' "deployed 1.4.2 to prod"
```

```bash
zirc watch --filter 'select(.message.event.kind == "x-acme/deploy")'
```

`zirc reference` documents what bots, scripts and agents build on: the team's IRC server,
Zirc's messages over it, and zircd's socket, whose protocol is versioned
([crates/zirc/src/reference.md](../crates/zirc/src/reference.md)), and
[generated/schema.json](../generated/schema.json), generated from Zirc's own types, describes it
for any language.

The `zirc login` handshake's types are in `zirc-proto` (`login`), and the web app's
TypeScript is generated from them:

```bash
bazel run //crates/zirc-proto:typescript
```

`zirc daemon install` runs zircd at login (launchd on macOS, systemd on Linux) and keeps
it running; `zirc daemon start` runs it until it's stopped instead.

## With coding agents

Several sessions on one machine can work side by side: give each its own worktree and
`zirc repo add` it, and zircd tells them when two worktrees change the same files
(`overlap: worktrees ~/src/api [main] and ~/src/api/.claude/worktrees/login [login] are
both changing src/auth.rs`), and tells each what the others post that concerns it.

Zirc comes with instructions for coding agents: start watching, keep `zirc watch`
running in the background, react to overlaps, and say what they're working on, treating
everything they hear as information rather than instructions. For Claude Code, install
them as a skill, which `zirc skill install` keeps in step with the CLI:

```bash
zirc skill install
```

A skill is read when it seems relevant, so also tell sessions to use it from a project's
`CLAUDE.md` or `AGENTS.md`, as this repository's `AGENTS.md` does. Agents without skills
can be given the same text inline:

```bash
zirc agent-guide >> AGENTS.md
```

## In the terminal

`zirc tui` is a chat client for your team's server, for people rather than machines: it
makes its own connection, joins every channel the server lists, and loads each one's
recent history. Conversations are down the left with unread counts (highlighted when you're
mentioned), the channel's members down the right, and Zirc's events read as one line each,
such as `◆ mattias-mbp/cc-1531f8 committed on login: Refresh tokens`.

When the connection is lost, the status line says so and when it tries again, and it
comes back by itself, into its channels with what was said meanwhile. A connection that
dies silently, as after the machine sleeps or the network changes, is noticed within a
minute, or within seconds of waking. Zirc's apps share this, in zirc-core's `connection`.

```bash
zirc tui
```

From a checkout, `bazel run //crates/zirc -- tui` runs it as built there. It used to be a
binary of its own, `zirc-tui`; installing or updating Zirc removes that.

The first time, it signs you in as yourself through zirc.dev, as the desktop app does: it
shows a link and a code, and once you confirm them in the browser it connects with your
account. The account's passphrase goes in the system's secret store (the macOS Keychain,
the Windows Credential Manager, or the Secret Service on Linux), under the `dev.zirc` service
Zirc's apps share, or in `~/.zirc/secrets.toml`, readable only by you, where
there's no such store; the rest goes in `~/.zirc/tui.toml`. Where your team's server has no
accounts yet, it connects with the server's password instead. Sign in again or out with:

```bash
zirc tui login
```

```bash
zirc tui logout
```

Signing in again to another organisation keeps the first: `zirc tui` then connects to each,
the status line names the one shown and what's unread in the others, and `/org [name]` or
Alt-O switches between them. `zirc tui logout --org <name>` signs out of one alone.

To connect another way, say how: `--account` with `ZIRC_TUI_PASSWORD` logs in to an
account of your choosing, `--config` connects to the server in Zirc's settings, and
`--server host[:port]` to another server; `--no-tls`, `--nick` and `--join` go with any of
them.

Type to talk; `/join`, `/part`, `/msg`, `/query`, `/me`, `/topic`, `/close` and `/quit` do
what they say, and `/help` lists them with the keys: Ctrl-N and Ctrl-P switch
conversations, Alt-A goes to the next unread one, PgUp and PgDn scroll, Tab completes
nicks, and Shift-Enter (Alt-Enter where the terminal can't tell it from Enter) starts a new
line. To react, `/react tada` (or `/react 🎉`) reacts to the newest message; Shift-↑ and
Shift-↓ pick another, and Alt-R or `/react` alone opens a picker to search. `/trim`
removes the message picked, or the newest, from the server's history, once you've said
`y`: yours, or anyone's in a channel you operate. `/away lunch`
says you're away, and `/away` alone that you're back. Guests, visitors to a public channel,
have a section of their own, their conversations shared with your team: in one, `/assign
dana` hands it to dana, `/close` resolves it, and an admin of the organisation can `/ban` them from every public channel, `/unban` them and `/invite` them to join.
These commands do the same in every zirc app, the web chat, desktop and phones included,
as zirc-chat carries them out (`Chat::run`); those only the terminal client has, such as
the picker, `/trim` and `/quote`, the others say are its.
Links show as their address's host and path, clickable in a terminal that takes OSC 8
hyperlinks (iTerm2, kitty, WezTerm, Ghostty, Windows Terminal and the like; not under tmux),
and as written elsewhere: `ZIRC_TUI_LINKS=plain` or `=labelled` says which. With the mouse, click a conversation to
show it, someone in the member list to see who they are, a message to pick it, or a reaction
to give or take back your own, and scroll with the wheel; hold Shift (Option in iTerm2) to
select text, or
start it with `--no-mouse` to leave the mouse to the terminal. It's dark on a dark terminal
and light on a light one; `--theme dark` or `--theme light` picks one.

## Usage analytics

Zirc's CLI, TUI and zircd send anonymous usage counts to [PostHog](https://posthog.com)'s EU
region: the app and its version, the operating system, each command's name (not its
arguments) and whether an agent ran it, which features are used (DMs, reactions, joins),
how often agents say what they're about to change and are told of overlaps and conflicts
ahead (counts only), connection failures by kind, and updates; an organisation's counts are
grouped by its opaque id. Never message text, nicks, account, channel or
repository names, paths or hostnames. The one exception: PostHog sees the IP address the
counts come from, and records roughly where that is. Each install has a random id, linked
to your account's opaque id once you sign in. The events are declared once, in
`crates/zirc-proto/src/telemetry.rs`; anything not there can't be sent.

The first interactive run says so in one line. To turn it off, set `telemetry = false` in
`~/.zirc/config.toml`: zircd reads it too. `ZIRC_TELEMETRY=0` or `DO_NOT_TRACK=1` in the
environment turns it off only for what runs with it, agents' runs among them, not for a
zircd that launchd or systemd starts at login. `posthog_key = ""` in `config.toml` sends nothing either, and
another key sends to that project instead of Zirc's. `ZIRC_TELEMETRY_INTERNAL=1` marks an
install as Zirc's own (CI, tests, the team's machines): it's remembered, and Zirc's dashboards
leave it out.
