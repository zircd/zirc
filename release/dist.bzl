"""Every platform's release archive from one machine: `:dist`, built for each platform in
bazel/platforms.bzl through a transition, as //release:publish publishes them."""

load("//bazel:platforms.bzl", "PLATFORMS")

def _each_platform_impl(_settings, _attr):
    return {
        platform: {
            "//command_line_option:compilation_mode": "opt",
            # As --config=release: releases run on macOS 11 and later.
            "//command_line_option:macos_minimum_os": "11.0",
            "//command_line_option:platforms": str(Label("//bazel/platforms:" + platform)),
        }
        for platform in PLATFORMS
    }

_each_platform = transition(
    implementation = _each_platform_impl,
    inputs = [],
    outputs = [
        "//command_line_option:compilation_mode",
        "//command_line_option:macos_minimum_os",
        "//command_line_option:platforms",
    ],
)

def _every_platform_impl(ctx):
    files = depset(transitive = [dist[DefaultInfo].files for dist in ctx.attr.dist])
    return [DefaultInfo(files = files, runfiles = ctx.runfiles(transitive_files = files))]

every_platform = rule(
    implementation = _every_platform_impl,
    doc = "`dist`, the archive and checksum for the machine building it, for every platform.",
    attrs = {
        "dist": attr.label(cfg = _each_platform, mandatory = True),
        "_allowlist_function_transition": attr.label(
            default = "@bazel_tools//tools/allowlists/function_transition_allowlist",
        ),
    },
)
