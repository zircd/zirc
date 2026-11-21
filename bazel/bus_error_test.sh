#!/usr/bin/env bash
# bus_error.c says where a Bus error struck: a program linked with it, dying of one on a file
# truncated under its mapping (bus_error_demo.c), names that file and still dies of the signal.
set -euo pipefail

file=$TEST_TMPDIR/truncated-shm
status=0
"$1" "$file" 2>"$TEST_TMPDIR/stderr" || status=$?
cat "$TEST_TMPDIR/stderr"
# 128 + SIGBUS (7): the default action, after the handler had its say.
[ "$status" -eq 135 ] || { echo "exited $status, not of SIGBUS" >&2; exit 1; }
grep -q '^bus error: SIGBUS at 0x' "$TEST_TMPDIR/stderr" || { echo "no address said" >&2; exit 1; }
grep -q "^bus error: in .* $file\$" "$TEST_TMPDIR/stderr" || { echo "the mapping isn't named" >&2; exit 1; }
