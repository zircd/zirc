#!/bin/sh
# Runs install.sh against a local copy of a release, as published:
#
#   install_test.sh <install.sh> <archive> <checksum>
#
# Each case runs with a home directory of its own and a PATH of only the system's tools.
set -eu

install_sh="$PWD/$1"
archive="$PWD/$2"
checksum="$PWD/$3"
name="$(basename "$archive" .tar.gz)"
version="${name#zirc-}"
version="${version%-*-*}"

tmp="${TEST_TMPDIR:-$(mktemp -d)}"
releases="$tmp/releases"
mkdir -p "$releases/v$version"
cp "$archive" "$checksum" "$releases/v$version/"
chmod u+w "$releases/v$version/"*
printf '%s\n' "$version" >"$releases/latest.txt"

fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

# install <case> [VAR=value...]: runs install.sh in a fresh home, its output in $out. Each
# `env -i` here passes ZIRC_TELEMETRY=0 on, which it would drop: tests aren't usage.
install() {
    home="$tmp/$1"
    shift
    mkdir -p "$home"
    out="$home/out"
    env -i HOME="$home" PATH=/usr/bin:/bin ZIRC_TELEMETRY=0 ZIRC_DOWNLOAD_URL="file://$releases" \
        ZIRC_APPLICATIONS="$home/system/Applications" "$@" sh "$install_sh" >"$out" 2>&1
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    else
        shasum -a 256 "$1" | cut -d ' ' -f 1
    fi
}

# publish <releases> <name> <directory>: an archive named <name> of <directory>, beside its
# checksum.
publish() {
    rm -rf "$tmp/staging"
    mkdir -p "$tmp/staging" "$1/v$version"
    cp -R "$3" "$tmp/staging/$2"
    tar -cf - -C "$tmp/staging" "$2" | gzip -n >"$1/v$version/$2.tar.gz"
    printf '%s  %s\n' "$(sha256 "$1/v$version/$2.tar.gz")" "$2.tar.gz" >"$1/v$version/$2.tar.gz.sha256"
}

expect() {
    grep -qF -- "$1" "$out" || fail "expected \"$1\" in:
$(cat "$out")"
}

# The latest release, into ~/.local/bin, which isn't on the PATH.
install latest || fail "install failed: $(cat "$out")"
for binary in zirc zircd zirc-remote; do
    [ -x "$home/.local/bin/$binary" ] || fail "no $binary in ~/.local/bin"
done
[ "$("$home/.local/bin/zirc" --version)" = "zirc $version" ] || fail "zirc isn't $version"
"$home/.local/bin/zirc" tui --help >/dev/null || fail "zirc has no tui"
grep -qF "version: $version" "$home/.claude/skills/zirc/SKILL.md" || fail "no zirc skill"
grep -qF "releases = \"file://$releases\"" "$home/.local/bin/.zirc-install.toml" ||
    fail "no receipt saying where zirc came from"
expect "Installed zirc, zircd and zirc-remote $version in ~/.local/bin"
# shellcheck disable=SC2016 # the profile line, as printed, with $HOME in it
expect 'export PATH="$HOME/.local/bin:$PATH"'
expect "signed in as yourself: zirc tui"
expect "isn't on your PATH"
expect "  zirc repo add"
if [ "$(uname -s)" != Darwin ]; then
    grep -qF "zirc app" "$out" && fail "offered the Mac app on $(uname -s)"
fi

# A version asked for, into a directory on the PATH, over an older install that had a
# zirc-tui, which goes: zirc tui took its place.
dir="$tmp/pinned/bin"
mkdir -p "$dir"
printf 'old\n' >"$dir/zirc"
printf 'old\n' >"$dir/zirc-tui"
install pinned ZIRC_VERSION="v$version" ZIRC_INSTALL_DIR="$dir" PATH="$dir:/usr/bin:/bin" ||
    fail "install failed: $(cat "$out")"
[ "$("$dir/zirc" --version)" = "zirc $version" ] || fail "zirc wasn't replaced"
grep -qF "isn't on your PATH" "$out" && fail "warned about a directory on the PATH"
[ ! -e "$dir/.zirc.new" ] || fail "left a temporary file behind"
[ ! -e "$dir/zirc-tui" ] || fail "left the old zirc-tui behind"

mkdir -p "$tmp/unpacked"
tar -xzf "$archive" -C "$tmp/unpacked"

# On a Mac, as uname says, with hdiutil mounting a disk image that's a tar file, and ditto
# copying.
stubs="$tmp/stubs"
mkdir -p "$stubs"
cat >"$stubs/uname" <<'STUB'
#!/bin/sh
case "$1" in
    -s) echo Darwin ;;
    -m) echo arm64 ;;
esac
STUB
cat >"$stubs/hdiutil" <<'STUB'
#!/bin/sh
printf '%s\n' "$*" >>"$HOME/hdiutil.log"
case "$1" in
    attach)
        while [ $# -gt 1 ]; do
            [ "$1" = -mountpoint ] && mount="$2"
            shift
        done
        tar -xf "$1" -C "$mount"
        ;;
    detach)
        for last; do :; done
        rm -rf "$last"
        ;;
esac
STUB
cat >"$stubs/ditto" <<'STUB'
#!/bin/sh
cp -pR "$1" "$2"
STUB
# Apple's tools, saying whose the app is: zirc's team unless STUB_TEAM says another.
cat >"$stubs/codesign" <<'STUB'
#!/bin/sh
[ "$1" = -dv ] && echo "TeamIdentifier=${STUB_TEAM:-7754LSCV7X}" >&2
exit 0
STUB
cat >"$stubs/spctl" <<'STUB'
#!/bin/sh
exit 0
STUB
chmod 755 "$stubs/uname" "$stubs/hdiutil" "$stubs/ditto" "$stubs/codesign" "$stubs/spctl"
mac="$tmp/mac-releases"
publish "$mac" "zirc-$version-macos-arm64" "$tmp/unpacked/$name"
printf '%s\n' "$version" >"$mac/latest.txt"
mkdir -p "$tmp/app/zirc.app/Contents" "$mac/desktop"
printf 'the zirc app\n' >"$tmp/app/zirc.app/Contents/Info.plist"
tar -cf "$mac/desktop/zirc.dmg" -C "$tmp/app" zirc.app
# As release/latest_json.py in zircapp/monorepo writes it: the Mac's release at the top, then
# other platforms' under "platforms", whose urls and checksums the installer must not take.
printf '{
  "version": "1.0.0",
  "url": "file://%s",
  "sha256": "%s",
  "minimum_macos": "11.0",
  "platforms": {
    "linux-x86_64": {
      "version": "1.0.0",
      "appimage": {"url": "file:///nonexistent/zirc.AppImage", "sha256": "%064d"},
      "tarball": {"url": "file:///nonexistent/zirc.tar.gz", "sha256": "%064d"}
    }
  }
}\n' "$mac/desktop/zirc.dmg" "$(sha256 "$mac/desktop/zirc.dmg")" 1 2 >"$mac/desktop/latest.json"
printf 'y\n' >"$tmp/yes"
printf 'n\n' >"$tmp/no"
# on_mac <case> [VAR=value...]
on_mac() {
    case_name="$1"
    shift
    mkdir -p "$tmp/$case_name/system/Applications"
    install "$case_name" PATH="$stubs:/usr/bin:/bin" ZIRC_DOWNLOAD_URL="file://$mac" "$@"
}
prompt="Also install the zirc app for Mac? [y/N]"

# Asked on a terminal and answered yes: the app, from a disk image mounted read-only.
on_mac mac-yes ZIRC_TTY="$tmp/yes" || fail "install failed: $(cat "$out")"
expect "$prompt"
expect "Installed the zirc app in ~/system/Applications"
grep -qF "the zirc app" "$home/system/Applications/zirc.app/Contents/Info.plist" ||
    fail "no zirc.app in Applications"
grep -q '^attach .*-readonly' "$home/hdiutil.log" || fail "didn't mount it read-only"
grep -q '^detach' "$home/hdiutil.log" || fail "didn't unmount it"
[ ! -e "$home/system/Applications/.zirc.app.new" ] || fail "left a temporary copy behind"

# Signed by someone else: not installed, and nothing of it left behind.
on_mac mac-unsigned ZIRC_APP=yes STUB_TEAM=EVIL000000 && fail "installed an app that isn't zirc's"
expect "isn't signed by zirc"
[ ! -e "$home/system/Applications/zirc.app" ] || fail "installed an app that isn't zirc's"
[ ! -e "$home/system/Applications/.zirc.app.new" ] || fail "left the unsigned copy behind"

# Answered no: nothing more.
on_mac mac-no ZIRC_TTY="$tmp/no" || fail "install failed: $(cat "$out")"
expect "$prompt"
grep -qF "Installed the zirc app" "$out" && fail "installed the app after a no"
[ ! -e "$home/hdiutil.log" ] || fail "opened the disk image after a no"

# Installed already, in ~/Applications: not asked.
mkdir -p "$tmp/mac-installed/Applications/zirc.app"
on_mac mac-installed ZIRC_TTY="$tmp/yes" || fail "install failed: $(cat "$out")"
grep -qF "zirc app" "$out" && fail "offered an installed app: $(cat "$out")"

# Without a terminal, as for an agent: one line on how to get it, and no question.
on_mac mac-quiet || fail "install failed: $(cat "$out")"
expect "The zirc app for Mac is at"
expect "ZIRC_APP=yes"
grep -qF "$prompt" "$out" && fail "asked without a terminal"
[ ! -e "$home/system/Applications/zirc.app" ] || fail "installed the app without asking"

# ZIRC_APP=yes answers for it, into ~/Applications when Applications isn't writable. (Root
# can write anywhere.)
if [ "$(id -u)" != 0 ]; then
    mkdir -p "$tmp/mac-own/system/Applications"
    chmod 555 "$tmp/mac-own/system/Applications"
    on_mac mac-own ZIRC_APP=yes || fail "install failed: $(cat "$out")"
    grep -qF "$prompt" "$out" && fail "asked with ZIRC_APP set"
    expect "Installed the zirc app in ~/Applications"
    [ -d "$home/Applications/zirc.app" ] || fail "no zirc.app in ~/Applications"
    chmod 755 "$tmp/mac-own/system/Applications"
fi

# A disk image that doesn't match its checksum isn't opened.
printf '{"url":"file://%s","sha256":"%064d"}\n' "$mac/desktop/zirc.dmg" 0 >"$mac/desktop/latest.json"
on_mac mac-corrupt ZIRC_APP=yes && fail "installed an app that doesn't match its checksum"
expect "the zirc app doesn't match its SHA-256"
[ ! -e "$home/hdiutil.log" ] || fail "opened a disk image that doesn't match its checksum"
[ ! -e "$home/system/Applications/zirc.app" ] || fail "installed the app anyway"


# A release that isn't there.
install missing ZIRC_VERSION=0.0.0-missing && fail "installed a missing release"
expect "couldn't download"

# An archive that doesn't match its checksum installs nothing.
printf '%064d  %s\n' 0 "$name.tar.gz" >"$releases/v$version/$name.tar.gz.sha256"
install corrupt && fail "installed an archive that doesn't match its checksum"
expect "doesn't match its SHA-256"
[ ! -e "$tmp/corrupt/.local/bin/zirc" ] || fail "installed zirc anyway"

printf 'PASS\n'
