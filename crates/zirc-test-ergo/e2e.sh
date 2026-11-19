# shellcheck shell=bash
# The apps' end-to-end path for a driver in shell: zirc-test-ergo's `serve` and its control
# port (src/control.rs, which says what each ask answers; src/scenario.rs has the path).
# Sourced by an app's test, which works its app and asks here for the rest:
#
#   . "$(dirname "$0")/…/zirc-test-ergo/e2e.sh"
#   e2e_serve "$serve" "$work"          # a throwaway Ergo; sets E2E_CONTROL and the words
#   web=$(e2e_signin dana)              # the zirc.dev to point the app at
#   e2e_post /step/history              # the teammate's side, a step at a time
#   e2e_heard dana "hello from the app" # the server heard the app say it, or this fails
#   e2e_reacted dana 👍                 # … and react with it
#   e2e_post /restart                   # the server restarts, the teammate comes back
#   e2e_joined dana                     # … and so has the app
#
# Needs curl, and ZIRC_ERGO and ZIRC_ERGO_CONFIG as `serve` does. Bash 3.2, as macOS has.

# e2e_serve <serve> <work directory>: starts `serve` with its control port on a free port,
# and sets E2E_CONTROL to its address and the path's words (E2E_CHANNEL, E2E_TEAMMATE,
# E2E_HISTORY, E2E_INTENDED, E2E_REPLY, E2E_BACK and the rest of scenario.rs's Words). The
# server's lifetime is this shell's: `serve` stops, and its Ergo with it, when its input
# closes, which is when this shell ends, however it ends. Nothing to kill, nothing left behind.
e2e_serve() {
    local serve=$1 work=$2 waited=0
    : >"$work/serve.out"
    # File descriptor 9 is the writing end of serve's input, held open and never written to.
    exec 9> >(ZIRC_SERVE_CONTROL=0 "$serve" >"$work/serve.out" 2>"$work/serve.err")
    until grep -q '^control ' "$work/serve.out"; do
        waited=$((waited + 1))
        if [ "$waited" -gt 300 ]; then
            echo "e2e: serve didn't start its control port:" >&2
            cat "$work/serve.out" "$work/serve.err" >&2
            return 1
        fi
        sleep 0.1
    done
    E2E_CONTROL=$(sed -n 's/^control //p' "$work/serve.out")
    E2E_WORK=$work
    eval "$(curl -fsS "$E2E_CONTROL/words?sh")"
}

# e2e_post <path> [json]: asks the control port and prints its answer. Fails, saying what
# was answered, when that isn't 200.
e2e_post() {
    local path=$1 body=${2:-} status
    status=$(curl -sS -o "$E2E_WORK/answer.json" -w '%{http_code}' --max-time 120 \
        -X POST -H 'content-type: application/json' --data "$body" "$E2E_CONTROL$path") || status=000
    if [ "$status" != 200 ]; then
        echo "e2e: POST $path $body answered $status: $(cat "$E2E_WORK/answer.json" 2>/dev/null)" >&2
        return 1
    fi
    cat "$E2E_WORK/answer.json"
}

# e2e_signin <handle>: starts the stand-in zirc.dev that signs the app in as <handle>, and
# prints its address, for the app's ZIRC_WEB or its sign-in screen.
e2e_signin() {
    e2e_post /signin "{\"handle\":\"$1\"}" | sed -n 's/.*"web":"\([^"]*\)".*/\1/p'
}

# e2e_joined <nick> [seconds]: <nick> is in the path's channel on the server as it runs now,
# within so many seconds (60): after a restart, that the app is back.
e2e_joined() {
    e2e_post /joined "{\"nick\":\"$1\",\"channel\":\"$E2E_CHANNEL\",\"within\":${2:-60}}" >/dev/null
}

# e2e_heard <nick> <text>: the server heard <nick> say a message containing <text> in the
# path's channel, within 20 seconds.
e2e_heard() {
    e2e_post /heard "{\"from\":\"$1\",\"channel\":\"$E2E_CHANNEL\",\"text\":\"$2\"}" >/dev/null
}

# e2e_reacted <nick> <emoji>: the server heard <nick> react with <emoji> to a message in the
# path's channel, within 20 seconds.
e2e_reacted() {
    e2e_post /heard "{\"from\":\"$1\",\"channel\":\"$E2E_CHANNEL\",\"react\":\"$2\"}" >/dev/null
}
