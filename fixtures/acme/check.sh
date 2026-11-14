#!/usr/bin/env bash
# Runs check.py on the showcase, with the system's Python.
set -euo pipefail
exec python3 -I "$(dirname "$0")/check.py" "$1"
