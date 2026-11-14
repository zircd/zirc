"""Build a binary as a container runs it, whatever the build was asked for."""

def _release_transition_impl(_settings, attr):
    return {
        "//command_line_option:compilation_mode": "opt",
        "//command_line_option:platforms": "@zig_sdk//platform:" + attr.platform,
    }

_release_transition = transition(
    implementation = _release_transition_impl,
    inputs = [],
    outputs = [
        "//command_line_option:compilation_mode",
        "//command_line_option:platforms",
    ],
)

def _release_binary_impl(ctx):
    return [DefaultInfo(files = depset(ctx.files.src))]

release_binary = rule(
    implementation = _release_binary_impl,
    doc = """The binary `src`, optimised (-c opt, which also strips its debug information) and
for Linux on `platform`, for a container image: zirc's cluster's linux_amd64 nodes, or a
self-hosted server's machine. Its own build stays as it was, so tests keep building quickly.""",
    attrs = {
        "src": attr.label(cfg = _release_transition, mandatory = True),
        "platform": attr.string(
            default = "linux_amd64",
            values = ["linux_amd64", "linux_arm64"],
            doc = "zig's platform to build for.",
        ),
    },
)
