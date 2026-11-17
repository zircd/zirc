#!/bin/sh
# Installs zirc's released binaries, zirc, zircd and zirc-remote:
#
#   curl -fsSL https://zirc.dev/install | sh
#
# Settings, from the environment:
#   ZIRC_VERSION      the release to install, such as 0.1.0. The latest unless set.
#   ZIRC_INSTALL_DIR  where to put the binaries. ~/.local/bin unless set.
#   ZIRC_DOWNLOAD_URL where releases are published. zirc's bucket unless set.
#   ZIRC_APP          yes or no: whether to install the zirc app on a Mac without asking.
#
# It checks each download against its published SHA-256, replaces older binaries, installs
# the zirc skill for Claude Code, and restarts zircd if this machine is set up already. It
# leaves .zirc-install.toml beside the binaries, naming where they came from, so they update
# themselves from there: zircd about once a day, and `zirc update` when asked, which also check
# each release's signature (crates/zirc-core/src/update.rs). This script doesn't: it comes
# from the same bucket as the releases, so whoever could replace a release could replace it
# too, and a shell has no portable way to check an Ed25519 signature. On a Mac
# without the zirc app, it offers to install that too, when run in a terminal, and installs
# it only if Apple's tools say it's signed by zirc and notarised.
#
# For tests: ZIRC_APPLICATIONS is the folder the app goes in before ~/Applications, and
# ZIRC_TTY the terminal to ask on.
set -eu

download_url="${ZIRC_DOWNLOAD_URL:-https://storage.googleapis.com/zirc-releases}"
install_dir="${ZIRC_INSTALL_DIR:-$HOME/.local/bin}"
zirc_home="${ZIRC_HOME:-$HOME/.zirc}"
applications="${ZIRC_APPLICATIONS:-/Applications}"

say() {
    printf '%s\n' "$*"
}

# A path for people: under the home directory, as ~/…
shown() {
    case "$1" in
        "$HOME"/*) printf '~%s\n' "${1#"$HOME"}" ;;
        *) printf '%s\n' "$1" ;;
    esac
}

die() {
    printf 'zirc install: %s\n' "$*" >&2
    exit 1
}

# This machine, by the names release archives use: <os>-<arch>.
platform() {
    case "$(uname -s)" in
        Darwin) os=macos ;;
        Linux) os=linux ;;
        *) die "zirc has no release for $(uname -s)" ;;
    esac
    case "$(uname -m)" in
        arm64 | aarch64) arch=arm64 ;;
        x86_64 | amd64) arch=x86_64 ;;
        *) die "zirc has no release for $(uname -m)" ;;
    esac
    # A shell running under Rosetta says x86_64 on an Apple silicon Mac.
    if [ "$os-$arch" = macos-x86_64 ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null)" = 1 ]; then
        arch=arm64
    fi
    printf '%s-%s\n' "$os" "$arch"
}

fetch() {
    curl --proto '=https,file' --tlsv1.2 -fsSL "$1" -o "$2" || die "couldn't download $1"
}

# mac_field <name> <latest.json>: the Mac's release's field, a string: the first one named so,
# as the Mac's release comes before "platforms", where other platforms' have the same names.
# One key to a line first, so it doesn't matter how the file is laid out.
mac_field() {
    tr ',{}' '[\n*]' <"$2" |
        sed -n "s/^[[:space:]]*\"$1\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" | head -n 1
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    else
        shasum -a 256 "$1" | cut -d ' ' -f 1
    fi
}

# Whether the app at $1 is zirc's own, as Apple's tools say: signed whole by zirc's Developer
# ID team (oss/apps/ios/BUILD.bazel's TEAM_ID: change both) and notarised. curl leaves no
# quarantine mark on what it downloads, so Gatekeeper wouldn't look when it's first opened.
signed_by_zirc() {
    codesign --verify --deep --strict "$1" >/dev/null 2>&1 || return 1
    codesign -dv "$1" 2>&1 | grep -q '^TeamIdentifier=7754LSCV7X$' || return 1
    spctl --assess --type execute "$1" >/dev/null 2>&1
}

# Everything it does, in one function called on the script's last line: cut off part-way
# under curl | sh, a shell would otherwise run the part that arrived.
main() {
    command -v curl >/dev/null 2>&1 || die "curl is needed to download zirc"

    target="$(platform)"
    tmp="$(mktemp -d)"
    mounted=""
    cleanup() {
        [ -z "$mounted" ] || hdiutil detach -quiet "$mounted" >/dev/null 2>&1 || true
        rm -rf "$tmp"
    }
    trap cleanup EXIT

    if [ -n "${ZIRC_VERSION:-}" ]; then
        version="${ZIRC_VERSION#v}"
    else
        fetch "$download_url/latest.txt" "$tmp/latest.txt"
        version="$(tr -d '[:space:]' <"$tmp/latest.txt")"
        [ -n "$version" ] || die "$download_url/latest.txt names no release"
    fi

    name="zirc-$version-$target"
    say "Downloading zirc $version for $target"
    fetch "$download_url/v$version/$name.tar.gz" "$tmp/$name.tar.gz"
    fetch "$download_url/v$version/$name.tar.gz.sha256" "$tmp/$name.tar.gz.sha256"
    expected="$(cut -d ' ' -f 1 <"$tmp/$name.tar.gz.sha256")"
    actual="$(sha256 "$tmp/$name.tar.gz")"
    [ "$expected" = "$actual" ] || die "$name.tar.gz doesn't match its SHA-256: expected $expected, got $actual"

    # Releases packed on a Mac before 10 Nov 2026 hold their files' extended attributes, which GNU
    # tar warns about, a line a file: quiet that there (package.sh leaves them out since).
    if tar --version 2>/dev/null | grep -q 'GNU tar'; then
        tar --warning=no-unknown-keyword -xzf "$tmp/$name.tar.gz" -C "$tmp"
    else
        tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
    fi
    mkdir -p "$install_dir"
    # The binaries a release has (crates/zirc-core/src/update.rs BINARIES: change both).
    for binary in zirc zircd zirc-remote; do
        [ -f "$tmp/$name/$binary" ] || die "$name.tar.gz has no $binary"
        # Copy, then rename over the old one: a running zircd keeps its own file, and macOS
        # doesn't kill it for a binary changed underneath it.
        cp "$tmp/$name/$binary" "$install_dir/.$binary.new"
        chmod 755 "$install_dir/.$binary.new"
        mv -f "$install_dir/.$binary.new" "$install_dir/$binary"
    done
    # Older releases had a zirc-tui: zirc tui took its place.
    rm -f "$install_dir/zirc-tui"
    # What `zirc update` and zircd read, to update themselves from the same releases.
    {
        say "# zirc's installer put zirc here: it updates itself from these releases."
        say "releases = \"$download_url\""
    } >"$install_dir/.zirc-install.toml"
    zirc="$install_dir/zirc"
    say "Installed zirc, zircd and zirc-remote $version in $(shown "$install_dir")"

    case ":$PATH:" in
        *":$install_dir:"*) on_path=yes ;;
        *)
            on_path=no
            say ""
            say "$(shown "$install_dir") isn't on your PATH. Add it, for example in your shell's profile:"
            case "$install_dir" in
                "$HOME"/*) say "  export PATH=\"\$HOME${install_dir#"$HOME"}:\$PATH\"" ;;
                *) say "  export PATH=\"$install_dir:\$PATH\"" ;;
            esac
            ;;
    esac

    say ""
    "$zirc" skill install

    say ""
    if [ -f "$zirc_home/config.toml" ]; then
        # Set up already: run the new zircd, at login from now on. A zircd started by hand
        # stops first, so only one holds the connection.
        "$zirc" daemon stop >/dev/null 2>&1 || true
        "$zirc" daemon install
        say "zircd $version is running. Check it with: zirc status"
    else
        [ "$on_path" = yes ] || say "Once $(shown "$install_dir") is on your PATH:"
        say "Next, in a repository you work in (it signs you in at zirc.dev the first time):"
        say "  zirc repo add"
        say ""
        say "For a server of your own instead: zirc init --server <host>, then zirc daemon install"
    fi
    say ""
    say "Chat in the terminal, signed in as yourself: zirc tui"

    # The zirc app, on a Mac that doesn't have it, if the person running this wants it.
    app() {
        if [ -e "$applications/zirc.app" ] || [ -e "$HOME/Applications/zirc.app" ]; then
            return
        fi
        answer="${ZIRC_APP:-}"
        if [ -z "$answer" ]; then
            # Asked on the terminal: under curl | sh, the script's input is the download.
            if [ -n "${ZIRC_TTY:-}" ]; then
                tty="$ZIRC_TTY"
            elif [ -t 1 ] && (: </dev/tty) 2>/dev/null; then
                tty=/dev/tty
            else
                say ""
                say "The zirc app for Mac is at https://zirc.dev/download/mac, or install it with: curl -fsSL https://zirc.dev/install | ZIRC_APP=yes sh"
                return
            fi
            say ""
            printf 'Also install the zirc app for Mac? [y/N] '
            read -r answer <"$tty" || answer=""
        fi
        case "$answer" in
            [yY] | [yY][eE][sS]) ;;
            *) return ;;
        esac

        say "Downloading the zirc app"
        fetch "$download_url/desktop/latest.json" "$tmp/latest.json"
        expected="$(mac_field sha256 "$tmp/latest.json")"
        [ -n "$expected" ] || die "$download_url/desktop/latest.json has no sha256"
        # From the same file as its checksum, so the two always describe one release.
        url="$(mac_field url "$tmp/latest.json")"
        [ -n "$url" ] || die "$download_url/desktop/latest.json has no url"
        fetch "$url" "$tmp/zirc.dmg"
        actual="$(sha256 "$tmp/zirc.dmg")"
        [ "$expected" = "$actual" ] || die "the zirc app doesn't match its SHA-256: expected $expected, got $actual"

        mkdir "$tmp/mount"
        hdiutil attach -readonly -nobrowse -quiet -mountpoint "$tmp/mount" "$tmp/zirc.dmg" >/dev/null ||
            die "couldn't open the zirc app's disk image"
        mounted="$tmp/mount"
        [ -d "$mounted/zirc.app" ] || die "the zirc app's disk image has no zirc.app"
        # /Applications if this account can write there, never with sudo.
        folder="$applications"
        if [ ! -w "$folder" ]; then
            folder="$HOME/Applications"
            mkdir -p "$folder"
        fi
        ditto "$mounted/zirc.app" "$folder/.zirc.app.new"
        if ! signed_by_zirc "$folder/.zirc.app.new"; then
            rm -rf "$folder/.zirc.app.new"
            die "the zirc app downloaded isn't signed by zirc and notarised: it wasn't installed"
        fi
        mv "$folder/.zirc.app.new" "$folder/zirc.app"
        hdiutil detach -quiet "$mounted" >/dev/null 2>&1 || true
        mounted=""
        say "Installed the zirc app in $(shown "$folder")"
    }

    if [ "${target%-*}" = macos ]; then
        app
    fi
}

main "$@"
