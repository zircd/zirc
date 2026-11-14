#!/usr/bin/env bash
# Runs a test inside zirc-sandbox (bazel/sandbox.bzl): sandbox_test.sh <sandbox> <command>.
set -euo pipefail
exec "$1" -- "$2"
