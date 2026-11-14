#!/usr/bin/env bash
# Brings terminals.json up to date with what zirc and zircd print now: runs
# //crates/zirc:zirc_terminals_test, which leaves what it printed in its outputs when that
# differs from the stored copy, and copies that in. Run
#
#   bazel run //fixtures/home:terminals
#
# then look at the diff, and commit it with whatever changed what zirc prints.
set -euo pipefail
cd "${BUILD_WORKSPACE_DIRECTORY:?run with bazel run //fixtures/home:terminals}"
# One local build at a time, light: the Mac runs CI's jobs too.
bazel test //crates/zirc:zirc_terminals_test --jobs=4 --test_output=errors || true
outputs="$(bazel info bazel-testlogs)/oss/crates/zirc/zirc_terminals_test/test.outputs"
made="$outputs/terminals.json"
if [ ! -f "$made" ] && [ -f "$outputs/outputs.zip" ]; then
    unzip -o -q "$outputs/outputs.zip" terminals.json -d "$outputs"
fi
if [ -f "$made" ]; then
    cat "$made" > fixtures/home/terminals.json
    echo "fixtures/home/terminals.json"
else
    echo "terminals.json is up to date"
fi
