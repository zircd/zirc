"""zirc's Rust rules: every crate's BUILD file is one call, with its dependencies, crates.io
and zirc's own alike, read from its Cargo.toml, so they're declared in one place for Cargo
and Bazel."""

load("@crates//:defs.bzl", "aliases", "all_crate_deps")
load("@rules_rust//rust:defs.bzl", "rust_binary", "rust_library", "rust_shared_library", "rust_test")
load("@rules_rust_wasm_bindgen//:defs.bzl", "rust_wasm_bindgen")
load("@zirc_version//:deps.bzl", "ZIRC_DEPS")
load("@zirc_version//:version.bzl", "VERSION")
load(":wasm.bzl", "for_wasm")

# What tests that start a server need: the pinned Ergo and its default.yaml, found by
# zirc-test-ergo through these variables.
ERGO_DATA = ["//dev/ergo:ergo", "//dev/ergo:default_yaml"]
ERGO_ENV = {
    "ZIRC_ERGO": "$(rootpath //dev/ergo:ergo)",
    "ZIRC_ERGO_CONFIG": "$(rootpath //dev/ergo:default_yaml)",
}

# And what says where a Bus error in one of them struck (bus_error.c).
ERGO_DEPS = ["//bazel:bus_error"]

def _tests(name, surface, crate, deps, test_deps, ergo, test_data, test_env, test_size, test_tags = []):
    """The crate's unit tests, and each file in tests/ as an integration test of its own,
    as Cargo builds them. Those that start Ergo are medium-sized: a server and the daemons
    talking to it, besides the test. A crate whose tests start Ergo may start it in a unit
    test too, as for a part that isn't public.

    Each says which collection of tests it's in (test-results' collections, by these tags):
    its crate's surface, and its kind. The crate's own tests are unit tests; a file in
    tests/ is an integration test, several pieces in one process, unless its crate's tests
    start Ergo or are tagged e2e: then it's end to end, against a real server."""
    whole = "kind:end-to-end" if ergo or "e2e" in test_tags else "kind:integration"
    rust_test(
        name = name + "_test",
        crate = crate,
        aliases = aliases(normal_dev = True, proc_macro_dev = True),
        deps = all_crate_deps(normal_dev = True) + test_deps,
        link_deps = ERGO_DEPS if ergo else [],
        proc_macro_deps = all_crate_deps(proc_macro_dev = True),
        lint_config = "//bazel:lints",
        version = VERSION,
        data = ERGO_DATA if ergo else [],
        env = ERGO_ENV if ergo else {},
        size = "medium" if ergo else test_size,
        tags = ["surface:" + surface, "kind:unit"],
    )
    for src in native.glob(["tests/*.rs"], allow_empty = True):
        rust_test(
            name = name + "_" + src[len("tests/"):-len(".rs")] + "_test",
            srcs = [src] + native.glob(["tests/common/**/*.rs"], allow_empty = True),  # tests/common/mod.rs: what the tests share
            crate_root = src,
            aliases = aliases(normal = True, normal_dev = True, proc_macro = True, proc_macro_dev = True),
            deps = [crate] + deps + test_deps + all_crate_deps(normal = True, normal_dev = True),
            link_deps = ERGO_DEPS if ergo else [],
            proc_macro_deps = all_crate_deps(proc_macro = True, proc_macro_dev = True),
            lint_config = "//bazel:lints",
            version = VERSION,
            data = (ERGO_DATA if ergo else []) + test_data,
            env = (ERGO_ENV if ergo else {}) | test_env,
            size = "medium" if ergo else test_size,
            tags = test_tags + ["surface:" + surface, whole],
        )

def zirc_rust_library(name, surface, ergo = False, wasm = False, wasm_bindgen = False, test_data = [], test_env = {}, test_size = None, test_tags = [], compile_data = [], **kwargs):
    """A library crate, its unit tests and its integration tests.

    Args:
        name: the crate's name, as in its Cargo.toml.
        surface: the part of the product its tests are of, for test-results' collections:
            ios, android, mac, web-chat, terminal, app-core, cli, daemon, server, site,
            install or ci (ops/check.sh says when a test names none of them).
        ergo: whether its integration tests start an Ergo server.
        wasm: whether it builds for the browser too, as `<name>_wasm`, which the web
            client needs of the crates it runs.
        wasm_bindgen: whether it's the web client's way into Rust, as `<name>_bindgen`:
            the crate as a .wasm module and wasm-bindgen's ES module and TypeScript
            declarations for it, for the page to import.
        test_data: more files its integration tests need, such as other binaries.
        test_env: environment for its integration tests, to find test_data by.
        test_size: the Bazel size of its unit tests and of its integration tests that
            start no Ergo, for a crate whose tests are measured: "small" fails one that hangs
            within a minute, not the five a medium test gets. Those starting Ergo are medium.
        test_tags: tags for its integration tests, such as "e2e" for those that run on
            master's schedule rather than in each landing.
        compile_data: more files it includes with include_str! or include_bytes!, besides
            the Markdown in src.
        **kwargs: passed to rust_library.
    """
    deps, test_deps = ZIRC_DEPS[name]["deps"], ZIRC_DEPS[name]["test_deps"]
    rust_library(
        name = name,
        srcs = native.glob(["src/**/*.rs"]),
        compile_data = native.glob(["src/**/*.md"], allow_empty = True) + compile_data,
        aliases = aliases(),
        deps = all_crate_deps(normal = True) + deps,
        proc_macro_deps = all_crate_deps(proc_macro = True),
        lint_config = "//bazel:lints",
        version = VERSION,
        visibility = ["//visibility:public"],
        **kwargs
    )
    _tests(name, surface, ":" + name, deps, test_deps, ergo, test_data, test_env, test_size, test_tags)
    if wasm:
        for_wasm(name = name + "_wasm", crate = ":" + name)
    if wasm_bindgen:
        rust_shared_library(
            name = name + "_cdylib",
            crate_name = name.replace("-", "_"),
            srcs = native.glob(["src/**/*.rs"]),
            compile_data = native.glob(["src/**/*.md"], allow_empty = True) + compile_data,
            aliases = aliases(),
            deps = all_crate_deps(normal = True) + deps,
            proc_macro_deps = all_crate_deps(proc_macro = True),
            lint_config = "//bazel:lints",
            version = VERSION,
        )
        rust_wasm_bindgen(
            name = name + "_bindgen",
            # Function names and the toolchain's versions are a quarter of the module, and
            # nothing a page needs: without them zirc-web's is 2.6 MB, not 3.3 (gzip, 829 KB,
            # not 893). Panics still say what went wrong, without names in their stack.
            bindgen_flags = ["--remove-name-section", "--remove-producers-section"],
            out_name = name.replace("-", "_"),
            target = "web",
            wasm_file = ":" + name + "_cdylib",
            visibility = ["//visibility:public"],
        )

def zirc_rust_binary(name, surface, ergo = False, test_data = [], test_env = {}, test_size = None, compile_data = [], **kwargs):
    """A binary crate and its tests. When it has a src/lib.rs, as Cargo allows, the library
    is `<name>_lib` and the binary is built from src/main.rs on top of it.

    Args:
        name: the binary's name, as in its Cargo.toml.
        surface: the part of the product its tests are of, for test-results' collections:
            ios, android, mac, web-chat, terminal, app-core, cli, daemon, server, site,
            install or ci (ops/check.sh says when a test names none of them).
        ergo: whether its integration tests start an Ergo server.
        test_data: more files its integration tests need, such as other binaries.
        test_env: environment for its integration tests, to find test_data by.
        test_size: the Bazel size of its unit tests and of its integration tests that
            start no Ergo, for a crate whose tests are measured: "small" fails one that hangs
            within a minute, not the five a medium test gets. Those starting Ergo are medium.
        compile_data: more files it includes with include_str! or include_bytes!, besides
            the Markdown in src.
        **kwargs: passed to rust_binary.
    """
    deps, test_deps = ZIRC_DEPS[name]["deps"], ZIRC_DEPS[name]["test_deps"]
    common = dict(
        aliases = aliases(),
        # Files the crate includes with include_str!, such as zirc's agent guide.
        compile_data = native.glob(["src/**/*.md"], allow_empty = True) + compile_data,
        proc_macro_deps = all_crate_deps(proc_macro = True),
        lint_config = "//bazel:lints",
        version = VERSION,
    )
    if native.glob(["src/lib.rs"], allow_empty = True):
        rust_library(
            name = name + "_lib",
            crate_name = name.replace("-", "_"),
            srcs = native.glob(["src/**/*.rs"], exclude = ["src/main.rs"]),
            deps = all_crate_deps(normal = True) + deps,
            visibility = ["//visibility:public"],
            **common
        )
        rust_binary(
            name = name,
            srcs = ["src/main.rs"],
            deps = [":" + name + "_lib"] + all_crate_deps(normal = True) + deps,
            visibility = ["//visibility:public"],
            **(common | kwargs)
        )
        _tests(name, surface, ":" + name + "_lib", deps, test_deps, ergo, test_data, test_env, test_size)
    else:
        rust_binary(
            name = name,
            srcs = native.glob(["src/**/*.rs"]),
            deps = all_crate_deps(normal = True) + deps,
            visibility = ["//visibility:public"],
            **(common | kwargs)
        )
        _tests(name, surface, ":" + name, deps, test_deps, ergo, test_data, test_env, test_size)
