#!/bin/sh
# The programs that ship say zirc's version: each alone is compiled with it and says it as it
# starts, for its libraries to read (zirc_proto::version), so one that didn't would say 0.0.0
# to zirc update, in its requests and in the skill it installs.
# POSIX sh, as this package's scripts are.
#
#   versions_test.sh <zirc> <zircd> <version>
set -eu

zirc=$1 zircd=$2 version=$3
fail() { echo "FAIL: $*" >&2; exit 1; }

[ "$("$zirc" --version)" = "zirc $version" ] || fail "zirc --version says $("$zirc" --version)"
[ "$("$zircd" --version)" = "zircd $version" ] || fail "zircd --version says $("$zircd" --version)"

# What a library reads: the skill zirc installs says the version it's of.
home=$(mktemp -d)
HOME=$home ZIRC_TELEMETRY=0 "$zirc" skill install >/dev/null
said=$(grep -m1 '^version: ' "$home/.claude/skills/zirc/SKILL.md" || true)
[ "$said" = "version: $version" ] || fail "the installed skill says '$said', not $version"
