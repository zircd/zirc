# zirc-sidecar

The sidecar that runs beside each zirc server's [Ergo](https://ergo.chat) in the same pod. Ergo
speaks IRC; the sidecar speaks everything else over HTTP, and logs in to Ergo as its operator on
loopback.

What it does:

- **Accounts** (`accounts`): the account endpoint zirc.dev's web app and the server's controller
  call: registering people's and machines' accounts, teams' channels, profiles, settings.
- **Signing in** (`signin`): checks devices' own keys for Ergo (its auth-script, over a Unix
  socket), so apps and agents sign in without zirc.dev, and keeps the server's login log on its
  own disk.
- **History** (`history`, `trim`): searches Ergo's own SQLite history on someone's behalf, under
  the organisation's choice of who may read what, and scrubs what was removed off the disk.
- **Push** (`pusher`): what reaches people's phones while their app is closed, decided here by
  zirc-chat's push rule (mentions, replies, reactions, direct messages, and conversations set to
  All, by each phone's own levels and mutes), sent encrypted for the phone to zirc.dev, which
  relays it. It hears each message as Ergo writes it to its history (file events, no polling),
  and phones register with it, and apps say what was read, over IRC as `zirc-pusher`. Ergo's
  own web push is off.
- **Usage and metrics** (`usage`, `exporter`): counts, never content, and Ergo's own metrics as
  OpenMetrics.
- **Roles** (`roles`, `nicks`): each of the organisation's roles, as zirc.dev defines them, as a
  nick of its own that speaks for the session holding its seat, wakes it for what its triggers
  match and for a direct message, and keeps what comes while it's vacant for the next holder.
  An IRC client that hears its channels live and catches up over `CHATHISTORY` when it joins.
- **Reports** (`items`): one open item per role, kind and key from the reports said in `#ops`
  (`zirc report`, CI, the alert bridge), each waking the role's holder once, with a reminder,
  until a landing names it in a `Fixes: report <id>` trailer or the holder closes it.
- **Roles' addresses** (`inbound`): `POST /in/<role>/<secret>` on the server's own host, where
  tools post to a role without passing zirc.dev. Sentry's errors, GitHub's and Linear's new
  issues and PagerDuty's incidents become reports (one open item per upstream issue), a tool's
  resolving closes the item, and anything else wakes the role as it was sent. Signed posts are
  checked where the server has the secret; only counts and outcomes are logged.
- **Teams and channels** (`teams`, `janitor`, `say`, `profiles`, `config`): making a
  server's channels, ops, posts by integrations, names and pictures, and its config file match
  what the controller asks for.

It's a program of its own, `zirc-sidecar`, and a library: zirc's tenant controller runs the same
`zirc_sidecar::run` from its own binary (`zirc-controller sidecar`) and has the Kubernetes side:
what a server's pod is made of, backups, rollouts. Nothing here knows about Kubernetes. Ergo's
config file for a server is made here too (`config::file`), from the release's `default.yaml`
and the server's settings, once whoever runs it has held them to its bounds.

Two things that aren't obvious from the code. The tests that start a real Ergo live in the
controller's `tests/ergo.rs`, not here, because they configure Ergo with the controller's
renderer. And a change here reaches tenants only once the controller's pinned sidecar image
is bumped, so keep what the sidecar and the controller say to each other readable by the
older side.

Licensed under either of Apache License 2.0 or MIT, at your option (`LICENSE-APACHE` and
`LICENSE-MIT`, beside zirc's README).
