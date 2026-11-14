# zirc-sidecar

The sidecar that runs beside each zirc server's [Ergo](https://ergo.chat) in the same pod. Ergo
speaks IRC; the sidecar speaks everything else over HTTP, and logs in to Ergo as its operator on
loopback.

What it does:

- **Accounts** (`accounts`): the account endpoint zirc.dev's web app and the server's controller
  call: registering people's and machines' accounts, teams' channels, profiles, guests, settings.
- **Signing in** (`signin`): checks devices' own keys for Ergo (its auth-script, over a Unix
  socket), so apps and agents sign in without zirc.dev, and keeps the server's login log on its
  own disk.
- **History** (`history`, `trim`): searches Ergo's own SQLite history on someone's behalf, under
  the organisation's choice of who may read what, and scrubs what was removed off the disk.
- **Gateway** (`gateway`, `guests`, `visitors`, `demo`): a WebSocket for the public channels'
  guests, who aren't IRC users, and the demo server's scripted cast.
- **Usage and metrics** (`usage`, `exporter`): counts, never content, and Ergo's own metrics as
  OpenMetrics.
- **Workflows** (`workflows`): runs the organisation's workflows (`zirc workflow deploy`), each as
  the nick its file names, an IRC client that hears its channels live and catches up over
  `CHATHISTORY` when it joins; nothing runs on a timer. Only where the controller says the
  server runs them (`spec.workflows`): elsewhere nothing connects.
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
