"""The lints the root Cargo.toml's [workspace.lints] says, for Bazel's apps, tools and services.

rules_rust can't read that table, so it's said again here, once: change the two together
(Cargo.toml says so too). zirc's crates are held to more, //bazel:lints."""

load("@rules_rust//rust:defs.bzl", "rust_lint_config")

def workspace_lints(name = "lints", visibility = None):
    """A package's lint config as Cargo's: clippy's pedantic group, less two, and no unsafe code.

    Args:
        name: the target, `lints` unless set.
        visibility: who else may use it.
    """
    rust_lint_config(
        name = name,
        # In this order: the flags follow it, and the allows must come after the group.
        clippy = {
            "pedantic": "warn",  # keep sorted: false
            "module_name_repetitions": "allow",
            "must_use_candidate": "allow",
        },
        rustc = {"unsafe_code": "deny"},
        visibility = visibility,
    )
