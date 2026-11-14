"""The machines zirc runs on, by the names its releases use, as `<os>-<arch>`. Each is a
config_setting in //bazel, for selecting what's built for the machine running the build."""

PLATFORMS = {
    "linux-arm64": ("linux", "aarch64"),
    "linux-x86_64": ("linux", "x86_64"),
    "macos-arm64": ("macos", "aarch64"),
    "macos-x86_64": ("macos", "x86_64"),
}
