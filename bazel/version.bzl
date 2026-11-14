"""What Bazel needs from Cargo.toml, so Cargo.toml stays the one place it's declared: zirc's
version, from `[workspace.package]`, and which of zirc's own crates each crate depends on.
rules_rust reads neither (crates_universe covers only crates.io ones), so this repository
rule does and offers them as `@zirc_version//:version.bzl`'s `VERSION` and
`@zirc_version//:deps.bzl`'s `ZIRC_DEPS`."""

def _sections(cargo_toml):
    """Each `[section]`'s lines, stripped, without comments or blank lines."""
    sections = {}
    section = None
    for line in cargo_toml.splitlines():
        line = line.partition("#")[0].strip()
        if not line:
            continue
        if line.startswith("["):
            section = line
            sections.setdefault(section, [])
        elif section:
            sections[section].append(line)
    return sections

def _key(line):
    """`zirc-core` in `zirc-core.workspace = true` or `zirc-core = { path = … }`."""
    return line.partition("=")[0].strip().partition(".")[0].strip()

def _workspace_version(sections):
    for line in sections.get("[workspace.package]", []):
        key, _, value = line.partition("=")
        if key.strip() == "version":
            return value.strip().strip("\"")
    fail("no version in [workspace.package] in Cargo.toml")

def _workspace_crates(sections):
    """zirc's own crates, by name, with their directory: the `path =` workspace
    dependencies."""
    crates = {}
    for line in sections.get("[workspace.dependencies]", []):
        _, found, value = line.partition("path")
        if found:
            crates[_key(line)] = value.partition("\"")[2].partition("\"")[0]
    return crates

def _label(rctx, root, name, path):
    """The Bazel target for depending on the crate: a binary crate's library is
    `<name>_lib`, as zirc_rust_binary names it."""
    main = root.get_child(path).get_child("src").get_child("main.rs")
    rctx.watch(main)
    if main.exists:
        return "//{}:{}_lib".format(path, name)
    if path.split("/")[-1] != name:
        return "//{}:{}".format(path, name)
    return "//" + path

def _members(cargo_toml):
    """The workspace's members, as `members = [...]` in `[workspace]` lists them, globs
    and all, in a table that may span lines."""
    text = cargo_toml.partition("[workspace]")[2].partition("members")[2].partition("[")[2].partition("]")[0]
    return [part.strip().strip("\"") for part in text.split(",") if part.strip()]

def _member_dirs(rctx, root, members):
    """Each member's directory, relative to the root, with a trailing /* expanded."""
    dirs = []
    for member in members:
        if member.endswith("/*"):
            parent = member[:-len("/*")]
            for dir in rctx.path(root.get_child(parent)).readdir():
                if dir.get_child("Cargo.toml").exists:
                    dirs.append(parent + "/" + dir.basename)
        else:
            dirs.append(member)
    return dirs

def _zirc_deps(sections, labels, dev):
    """The workspace crates a crate's Cargo.toml depends on, as labels."""
    found = [labels[_key(line)] for line in sections.get("[dependencies]", []) if _key(line) in labels]
    if dev:
        found += [
            labels[_key(line)]
            for line in sections.get("[dev-dependencies]", [])
            if _key(line) in labels and labels[_key(line)] not in found
        ]
    return found

def _zirc_version_impl(rctx):
    cargo_toml = rctx.path(rctx.attr.cargo_toml)
    root = cargo_toml.dirname
    workspace = _sections(rctx.read(cargo_toml))
    crates = _workspace_crates(workspace)
    labels = {name: _label(rctx, root, name, path) for name, path in crates.items()}
    deps = {}

    # Every crate under the members' crates directory (`crates/*` here, `crates/*` in
    # the public repository), by its directory's name, which is its package's name; one
    # nothing depends on needn't be a workspace dependency.
    members = _members(rctx.read(cargo_toml))
    for path in _member_dirs(rctx, root, [m for m in members if m.endswith("crates/*")]):
        sections = _sections(rctx.read(root.get_child(path).get_child("Cargo.toml")))
        deps[path.split("/")[-1]] = {
            kind: [labels[_key(line)] for line in sections.get(section, []) if _key(line) in labels]
            for kind, section in [("deps", "[dependencies]"), ("test_deps", "[dev-dependencies]")]
        }

    # And every workspace member, by its package, for zirc_crate_deps: what all_crate_deps
    # leaves out, as crates_universe covers crates.io's crates only.
    packages = {}
    for path in _member_dirs(rctx, root, members):
        sections = _sections(rctx.read(root.get_child(path).get_child("Cargo.toml")))
        packages[path] = {
            "normal": _zirc_deps(sections, labels, dev = False),
            "normal_dev": _zirc_deps(sections, labels, dev = True),
        }
    rctx.file("BUILD.bazel", "")
    rctx.file("version.bzl", "VERSION = \"{}\"\n".format(_workspace_version(workspace)))
    rctx.file("deps.bzl", """ZIRC_DEPS = {}

_PACKAGES = {}

def zirc_crate_deps(normal = False, normal_dev = False):
    \"\"\"The workspace crates (zirc's own, under oss/) this package's Cargo.toml depends on,
    as all_crate_deps gives the rest: its [dependencies], and its [dev-dependencies] too
    with normal_dev.\"\"\"
    found = _PACKAGES[native.package_name()]
    if normal_dev:
        return found["normal_dev"]
    return found["normal"] if normal else []
""".format(json.encode_indent(deps), json.encode_indent(packages)))

zirc_version = repository_rule(
    implementation = _zirc_version_impl,
    attrs = {"cargo_toml": attr.label(mandatory = True, allow_single_file = True)},
)
