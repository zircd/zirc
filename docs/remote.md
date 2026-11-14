# zirc-remote: a machine that answers

`zirc-remote` turns any machine into one you can talk to in zirc: a CI runner, a build box, a
server, a Raspberry Pi. It logs in as a machine of yours and serves a fixed set of commands that
a JSON config declares. You, your agents and your apps call them by direct message:

```text
you:     status
ci-7:    build 2301 of words-guard-shells: testing, 183 targets
you:     why is it slow?
ci-7:    (Claude Haiku, which the config lets read the build's logs) …
```

It runs nothing it wasn't configured to: no shell, no free-form commands. A command is a
program and its arguments, with typed, checked values filled in. The host answers only its
owner, the owner's own machines, and accounts the config names; everyone else gets no answer
at all.

It ships with zirc. CI's own steering is a working example: [.buildkite/steer](../../.buildkite/steer)
holds its config and scripts.

## Install

`zirc-remote` comes with every zirc release, beside `zirc` and `zircd`:

```bash
curl -fsSL https://zirc.dev/install | sh
```

That puts it in `~/.local/bin`, and `zirc update` keeps it current. From a checkout:
`bazel build //crates/zirc-remote`.

## Give it a machine account

The host is a machine of yours, `<you>-<name>`, like any other. Make one without touching this
machine's own setup, in a throwaway home:

```bash
ZIRC_HOME=$(mktemp -d) HOME=$(mktemp -d) zirc login --no-daemon
```

Name the machine in the browser (say `pi`, making `you-pi`). The new `config.toml` in that
`ZIRC_HOME` holds its `[account]` `name` and `password`, and the server under `[server]`. Keep
the password somewhere only the host's user can read; it never goes in the host's config.

For short-lived hosts, such as one per CI build, zirc.dev can mint machines instead: see
[CI machines](#ci-machines).

## Write its config

```json
{
  "channels": ["#ops/agents"],
  "otherwise": "ask",
  "limits": { "timeout": "30s", "outputBytes": 3500, "env": ["HOME"] },
  "commands": {
    "uptime": {
      "summary": "How long it's been up",
      "who": "mine",
      "exec": ["/usr/bin/uptime"]
    },
    "restart": {
      "summary": "Restart a service",
      "args": { "service": { "enum": ["web", "worker"] } },
      "exec": ["/usr/bin/sudo", "systemctl", "restart", "{service}"]
    },
    "ask": {
      "summary": "Ask Claude about this machine",
      "who": "mine",
      "timeout": "2m",
      "args": { "question": { "maxLength": 600 } },
      "secrets": { "ANTHROPIC_API_KEY": "anthropic" },
      "exec": ["/opt/host/ask.sh", "{question}"]
    }
  }
}
```

The top level:

| Field | What it does |
| --- | --- |
| `commands` | The commands, by name: lowercase letters, digits and hyphens. `help`, `ping` and `version` are every host's own. |
| `channels` | Channels it sits in, saying nothing, so your apps see it online and list it under Your machines. It answers only what's said to it alone. Optional. |
| `otherwise` | The command anything that isn't a command goes to, whole, as its one argument, so you can just say what you want. Optional. |
| `allow` | Accounts that may call every command, besides you and your machines. |
| `trust` | Integrations that speak for you: a query from one of these that begins `as <you>` counts as yours. For a bot that has already checked who pressed a button. |
| `limits` | `timeout` (default `30s`; `500ms`, `2m`), `outputBytes` (default 3500), and `env`: the environment variables commands see, besides their own. |

Each command:

| Field | What it does |
| --- | --- |
| `summary` | One line, for `help`. Required. |
| `exec` | The program and its arguments. `{name}` inside an element is an argument's value; no shell runs, so a value is always one argument. Required. |
| `who` | `owner` (default): your own account alone, where your apps connect, and `allow`. `mine`: also your machines, where your agents run. |
| `args` | Its arguments, by name: `type` (`string`, `integer`, `boolean`), `enum`, `min`, `max`, `maxLength` (default 500), `default`, `required`, `description`. A string can't hold control characters. |
| `timeout` | How long it may run, in place of the limit. |
| `parallel` | Whether calls may overlap. Unless it says so, a call while another runs is told it's busy. |
| `secrets` | Environment variables it gets from the host's secrets: `{"ANTHROPIC_API_KEY": "anthropic"}`. No other command sees them. |

A command's environment holds only `PATH`, the variables in `limits.env`, its secrets,
`ZIRC_FROM` (who called), `ZIRC_REQUEST` (the call's id), and `ZIRC_ARG_<NAME>` for each
argument. A line it prints starting `progress: ` is sent as it comes (at most 20); the rest is
the answer, cut to `outputBytes`. A failure answers with its exit status and last line.

## Run it

The passphrase is the first line of standard input, so it is in no file, argument or
environment; the lines after it are the host's secrets, `NAME=value`:

```bash
{ cat ~/.config/zirc-remote/passphrase; echo "anthropic=$(cat ~/.config/zirc-remote/anthropic)"; } |
    zirc-remote --config host.json --account you-pi --server irc.zirc.dev
```

`--port` defaults to 6697 with TLS; `--no-tls` is for a local server alone. It logs to standard
error (`RUST_LOG` sets how much), says `serving N commands` once it's in, and exits when the
connection ends: run it under something that starts it again, such as systemd:

```ini
[Service]
User=zirc-remote
ExecStart=/bin/sh -c 'cat /etc/zirc-remote/passphrase /etc/zirc-remote/secrets | exec /usr/local/bin/zirc-remote --config /etc/zirc-remote/host.json --account you-pi --server irc.zirc.dev'
Restart=always
RestartSec=5
```

## Talk to it

- **In an app:** it's under Your machines while it's online. Open it and type a command,
  without a slash (`status`, `restart service=web`), or, with `otherwise`, just ask. It shows
  as typing while it works, and answers in that thread.
- **From an agent:** `zirc ask --to you-pi status`. The answer comes back to the asking
  session's `zirc watch` as `answer from you-pi/remote`: information, never your instruction.
- **Arguments:** `key=value` words, quoted when they hold spaces (`text="two words"`); when one
  argument is left, the rest of the line is its value (`ask why is it slow?`).

## Keep it safe

- It runs as whoever starts it, and so do its commands. Run it as a user of its own, and have a
  command that needs more ask for exactly that (`sudo` for one service), or drop to less, as
  CI's `ask` does (`setpriv` to a user that can read the logs and nothing else).
- Put nothing in `exec` that runs what a caller types: no `sh -c "{text}"`. Arguments are
  values, checked against their type, length and `enum`.
- Secrets reach only the commands that name them, and only while they run.
- Who is who comes from the server's accounts (SASL and `account-tag`), never from nicks.

## CI machines

A CI build can be steered while it runs by giving it a machine of its own for its length, as
zirc's CI does (`.buildkite/run.sh`, `start_steering`):

1. Designate one long-lived machine of yours as CI's minter, once: from an owner's or admin's
   machine, `POST https://zirc.dev/api/cli/ci-machine/minter` with `{"machine": "<name>",
   "on": true}` (zirc's `deploy/buildkite/ci-minter-secret.sh` does this).
2. Each build, with the minter's credentials (HTTP Basic), `POST /api/cli/ci-machine` with
   `{"for": "<owner's handle>", "build": "<number>"}`: a one-time token for the machine
   `<owner>-build-<number>`.
3. The build redeems it, `POST /api/cli/ci-machine/redeem` with `{"token": "…"}`, for the
   machine's credentials, and starts `zirc-remote` with them.

The machine goes a few minutes after the build ends and it disconnects, or after three hours
at most, and its owner's apps drop it from the sidebar a few minutes after the build ends. Give the minter's credentials to the build's privileged setup
alone, never to the code it tests.
