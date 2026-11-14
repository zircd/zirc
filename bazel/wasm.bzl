"""A crate built for the browser, `wasm32-unknown-unknown`, as zirc_rust_library's
`wasm = True` asks: building it is the check that the crate still has no I/O, so the web
client can run it."""

def _wasm32_impl(_settings, _attr):
    return {"//command_line_option:platforms": str(Label("@rules_rust//rust/platform:wasm32"))}

_wasm32 = transition(
    implementation = _wasm32_impl,
    inputs = [],
    outputs = ["//command_line_option:platforms"],
)

def _for_wasm_impl(ctx):
    return [DefaultInfo(files = depset(transitive = [crate[DefaultInfo].files for crate in ctx.attr.crate]))]

for_wasm = rule(
    implementation = _for_wasm_impl,
    doc = "`crate`, built for wasm32-unknown-unknown.",
    attrs = {
        "crate": attr.label(cfg = _wasm32, mandatory = True),
        "_allowlist_function_transition": attr.label(
            default = "@bazel_tools//tools/allowlists/function_transition_allowlist",
        ),
    },
)

def _optimized_impl(_settings, _attr):
    return {"//command_line_option:compilation_mode": "opt"}

_optimized = transition(
    implementation = _optimized_impl,
    inputs = [],
    outputs = ["//command_line_option:compilation_mode"],
)

def _optimized_files_impl(ctx):
    return [DefaultInfo(files = depset(transitive = [src[DefaultInfo].files for src in ctx.attr.src]))]

optimized = rule(
    implementation = _optimized_files_impl,
    doc = "`src`'s files, built with -c opt whatever the build asks: the web client's wasm, " +
          "which unoptimised is several times the size.",
    attrs = {
        "src": attr.label(cfg = _optimized, mandatory = True),
        "_allowlist_function_transition": attr.label(
            default = "@bazel_tools//tools/allowlists/function_transition_allowlist",
        ),
    },
)
