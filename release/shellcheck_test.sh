#!/bin/sh
# Checks release's shell scripts with the pinned ShellCheck: shellcheck_test.sh <shellcheck> <script>...
set -eu
shellcheck="$1"
shift
"$shellcheck" --shell=sh --severity=style "$@"
