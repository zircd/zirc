#!/usr/bin/env bash
# Runs Acme's checker (../acme/check.py) on the home page's team, with the system's Python.
set -euo pipefail
exec python3 -I "$(dirname "$0")/../acme/check.py" "$1"
