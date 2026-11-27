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

def _optimized_impl(ctx):
    return [DefaultInfo(files = ctx.attr.src[DefaultInfo].files)]

optimized = rule(
    implementation = _optimized_impl,
    doc = """`src`'s files, built with -c opt whatever the build asks: the web client's wasm,
which unoptimised is several times the size.

Built in the exec configuration, which is -c opt, rather than the build's own with -c opt:
zirc.dev builds the web client in exec (web/BUILD.bazel's chat_site), and a second
configuration compiled zirc-web's crates for wasm twice in every build of both, six crates
from zirc-proto up (build 10792). Wasm is the same whatever machine builds it.""",
    attrs = {
        "src": attr.label(cfg = "exec", mandatory = True),
    },
)
