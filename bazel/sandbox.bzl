"""Tests that run inside zirc-sandbox: a throwaway Ergo and two machines' zircds, as the
SDKs' tests need (crates/zirc-sandbox)."""

load("@rules_shell//shell:sh_test.bzl", "sh_test")
load(":rust.bzl", "ERGO_DATA", "ERGO_ENV")

def zirc_sandbox_test(name, command, data = [], env = {}, **kwargs):
    """Runs `command`, an executable target, inside zirc-sandbox, passing if it does.

    Args:
        name: the test's name.
        command: the executable to run, with ZIRC_HOME, ZIRC_PEER_HOME, ZIRC_REPO and
            ZIRC_PEER_REPO set.
        data: more files it needs.
        env: more environment for it.
        **kwargs: passed to sh_test.
    """
    sh_test(
        name = name,
        srcs = ["//bazel:sandbox_test.sh"],
        args = [
            "$(rootpath //crates/zirc-sandbox)",
            "$(rootpath %s)" % command,
        ],
        data = ["//crates/zirc-sandbox", command] + ERGO_DATA + data,
        env = ERGO_ENV | env,
        **kwargs
    )
