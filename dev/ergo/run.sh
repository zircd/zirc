#!/usr/bin/env bash
# Runs the pinned Ergo with its release's own default.yaml, changing only what local
# development needs through Ergo's environment overrides. State (database, self-signed
# certificate) is kept in .zirc-dev/ergo at the repository root.
#
#   bazel run //dev/ergo:run
#
# Listens on 127.0.0.1:6667 (plaintext), :6697 (TLS, self-signed), and 127.0.0.1:8097 for
# WebSocket (subprotocol text.ircv3.net) from the web client's dev server.
set -euo pipefail

runfiles="${RUNFILES_DIR:-$0.runfiles}"
ergo="$runfiles/$1"
default_yaml="$runfiles/$2"

state="${BUILD_WORKSPACE_DIRECTORY:?run this with bazel run}/.zirc-dev/ergo"
mkdir -p "$state"
cd "$state"

# Always the pinned release's config, so upgrading Ergo upgrades it too.
cp -f "$default_yaml" ircd.yaml
chmod u+w ircd.yaml

export ERGO__LANGUAGES__ENABLED=false
export ERGO__NETWORK__NAME=zirc-dev
export ERGO__SERVER__NAME=irc.zirc.localhost
# Ergo merges this into default.yaml's listeners, so 6667 and 6697 stay.
export ERGO__SERVER__LISTENERS='{"127.0.0.1:8097": {"websocket": true}}'
# An operator, as a tenant's sidecar has one, to give made-up machines the host that says
# whose they are, machine/<owner>, when trying an app by hand: OPER admin zirc-dev-oper.
export ERGO__OPERS__ADMIN__PASSWORD='$2a$04$NsXfTGiFeYO3ZIPfWuyfOeQts220pwSCdh7GYpvl44v/Qf/h.7T62'
# The web client's Vite dev server (web/vite.config.ts), and no other page.
export ERGO__SERVER__WEBSOCKETS__ALLOWED_ORIGINS='["http://localhost:5173", "http://127.0.0.1:5173"]'

[[ -f fullchain.pem ]] || "$ergo" mkcerts --conf ircd.yaml --quiet
[[ -f ircd.db ]] || "$ergo" initdb --conf ircd.yaml --quiet

exec "$ergo" run --conf ircd.yaml
