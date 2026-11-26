#!/usr/bin/env bash
# Runs check.py on the team, with the system's Python.
set -euo pipefail
exec python3 -I "$(dirname "$0")/check.py" "$1"
