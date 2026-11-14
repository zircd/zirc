#!/bin/sh
# Updates a release from this checkout, installed by install.sh, to a newer one, as zirc
# update does and zircd does by itself, from a local copy of the releases:
#
#   update_test.sh <install.sh> <release_sign> <archive> <checksum>
#
# The newer releases are fakes: shell scripts that say they ran, then run this checkout's
# binaries, so what restarts after an update is a zircd that answers. Each is signed with a
# key made for the test, which ZIRC_UPDATE_TEST_KEY has the updater trust in place of zirc's
# release key, and which lets it take file:// releases.
set -eu

install_sh="$PWD/$1"
release_sign="$PWD/$2"
archive="$PWD/$3"
checksum="$PWD/$4"
name="$(basename "$archive" .tar.gz)"
version="${name#zirc-}"
version="${version%-*-*}"
target="${name#"zirc-$version-"}"

tmp="${TEST_TMPDIR:-$(mktemp -d)}"
# zirc's directories, where the socket is: short, since socket paths are.
homes="$(mktemp -d /tmp/zirc-update.XXXXXX)"
cleanup() {
    for dir in "$homes"/*/; do
        [ -S "$dir/zircd.sock" ] && "$real/zirc" --home "$dir" daemon stop >/dev/null 2>&1
    done
    rm -rf "$homes"
}
trap cleanup EXIT
# sh runs the EXIT trap only on exiting by itself: a test stopped for its timeout gets
# SIGTERM, which would otherwise leave its zircds running.
trap 'exit 1' HUP INT TERM

releases="$tmp/releases"
mkdir -p "$releases/v$version"
cp "$archive" "$checksum" "$releases/v$version/"
chmod u+w "$releases/v$version/"*
printf '%s\n' "$version" >"$releases/latest.txt"

# The test's own release key, and another that zirc doesn't trust.
"$release_sign" generate "$tmp/test.pub" >"$tmp/test.key"
"$release_sign" generate "$tmp/other.pub" >"$tmp/other.key"
test_key="$(cat "$tmp/test.pub")"

# sign <checksum> [key]: its signature, with the test's key unless another's is given.
sign() {
    env -i ZIRC_UPDATE_TEST_KEY="$(cat "${2:-$tmp/test}.pub")" "$release_sign" sign "$1" \
        <"${2:-$tmp/test}.key" >/dev/null
}
sign "$releases/v$version/$(basename "$checksum")"

# This checkout's binaries, where no update replaces them.
real="$tmp/real"
mkdir -p "$real"
tar -xzf "$archive" -C "$tmp"
cp "$tmp/$name/zirc" "$tmp/$name/zircd" "$tmp/$name/zirc-remote" "$real/"

fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

expect() {
    grep -qF -- "$1" "$out" || fail "expected \"$1\" in:
$(cat "$out")"
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    else
        shasum -a 256 "$1" | cut -d ' ' -f 1
    fi
}

# fake <version>: a release whose binaries say they ran, in ~/ran, then run this
# checkout's.
fake() {
    staged="$tmp/fake/zirc-$1-$target"
    mkdir -p "$staged" "$releases/v$1"
    for binary in zirc zircd zirc-remote; do
        # shellcheck disable=SC2016 # the script's own variables, expanded when it runs
        printf '#!/bin/sh\necho "%s %s $*" >>"$HOME/ran"\nexec "%s" "$@"\n' \
            "$binary" "$1" "$real/$binary" >"$staged/$binary"
        chmod 755 "$staged/$binary"
    done
    tar -cf - -C "$tmp/fake" "zirc-$1-$target" | gzip -n >"$releases/v$1/zirc-$1-$target.tar.gz"
    printf '%s  %s\n' "$(sha256 "$releases/v$1/zirc-$1-$target.tar.gz")" "zirc-$1-$target.tar.gz" \
        >"$releases/v$1/zirc-$1-$target.tar.gz.sha256"
    sign "$releases/v$1/zirc-$1-$target.tar.gz.sha256"
}

# setup <case> [config line]: this checkout's release, installed by install.sh in a home of
# its own, with zirc's directory set up for a server that isn't there.
setup() {
    home="$tmp/$1"
    zirc_home="$homes/$1"
    bin="$home/.local/bin"
    out="$home/out"
    mkdir -p "$home" "$zirc_home"
    env -i HOME="$home" PATH=/usr/bin:/bin ZIRC_TELEMETRY=0 ZIRC_HOME="$zirc_home" \
        ZIRC_DOWNLOAD_URL="file://$releases" ZIRC_VERSION="$version" ZIRC_APP=no \
        sh "$install_sh" >"$out" 2>&1 ||
        fail "install failed: $(cat "$out")"
    printf 'nick = "updater"\n%s\n[server]\nhost = "127.0.0.1"\nport = 1\ntls = false\n' \
        "${2:-}" >"$zirc_home/config.toml"
}

# zirc <binary> <args...>: runs a zirc as the case's machine, trusting the test's key, its
# output in $out. Each `env -i` here passes ZIRC_TELEMETRY=0 on, which it would drop: tests
# aren't usage.
zirc() {
    binary="$1"
    shift
    env -i HOME="$home" PATH=/usr/bin:/bin ZIRC_TELEMETRY=0 ZIRC_HOME="$zirc_home" \
        ${test_key:+ZIRC_UPDATE_TEST_KEY="$test_key"} \
        ${ZIRC_UPDATE_EVERY:+ZIRC_UPDATE_EVERY="$ZIRC_UPDATE_EVERY"} "$binary" "$@" >"$out" 2>&1
}

# eventually <what> <command...>: waits up to 20 seconds for the command to pass.
eventually() {
    what="$1"
    shift
    tries=0
    until "$@"; do
        tries=$((tries + 1))
        [ "$tries" -lt 200 ] || fail "$what"
        sleep 0.1
    done
}

# The installer says where these came from.
setup receipt
grep -qF "releases = \"file://$releases\"" "$bin/.zirc-install.toml" ||
    fail "no receipt: $(cat "$bin/.zirc-install.toml")"

# Up to date: said so, and fine.
zirc "$bin/zirc" update || fail "update failed: $(cat "$out")"
expect "zirc $version is up to date"
zirc "$bin/zirc" update --check || fail "update --check failed: $(cat "$out")"
expect "zirc $version is up to date"

fake 99.0.0
printf '99.0.0\n' >"$releases/latest.txt"

# --check only says so.
zirc "$bin/zirc" update --check || fail "update --check failed: $(cat "$out")"
expect "zirc 99.0.0 is out (this is $version)"
cmp -s "$bin/zirc" "$real/zirc" || fail "update --check changed zirc"

# zirc update replaces the binaries and restarts zircd, which then runs the new zircd.
setup by-hand "auto_update = false"
zirc "$bin/zirc" daemon start || fail "zircd didn't start: $(cat "$out")"
zirc "$bin/zirc" update || fail "update failed: $(cat "$out")"
expect "updated zirc $version → 99.0.0 in ~/.local/bin"
expect "zircd restarted, running $version"
grep -qF "zircd 99.0.0" "$home/ran" || fail "the new zircd didn't run"
for binary in zirc zircd; do
    grep -qF "$binary 99.0.0" "$bin/$binary" || fail "$binary wasn't replaced"
done
[ -z "$(find "$bin" -name '.*new*' -o -name '.zirc-update*')" ] || fail "left files behind in $bin"
grep -qF "releases = " "$bin/.zirc-install.toml" || fail "lost the receipt"

# zircd updates by itself, when due, and comes back as the new one; a watch running then
# keeps watching.
setup by-itself
ZIRC_UPDATE_EVERY=1
# Due in a few seconds, once the watch is running: long enough for a busy machine.
printf '{"checked":%s}\n' "$(($(date +%s) + 8))" >"$zirc_home/update.json"
zirc "$bin/zirc" daemon start || fail "zircd didn't start: $(cat "$out")"
env -i HOME="$home" PATH=/usr/bin:/bin ZIRC_TELEMETRY=0 ZIRC_HOME="$zirc_home" "$real/zirc" watch --all \
    >"$home/watch" 2>&1 &
watching=$!
watching() {
    zirc "$real/zirc" status && grep -qF "1 watch without a session" "$out"
}
eventually "zirc watch didn't start" watching
[ ! -e "$home/ran" ] || fail "zircd updated before the watch started: give it longer"
eventually "zircd didn't update itself: $(cat "$zirc_home/zircd.log")" \
    grep -qF "zircd 99.0.0" "$home/ran"
grep -qF "zirc 99.0.0" "$bin/zirc" || fail "zirc wasn't replaced"
grep -qF "installing zirc 99.0.0" "$zirc_home/zircd.log" || fail "zircd didn't log the update"
eventually "zirc watch didn't say zircd restarted: $(cat "$home/watch")" \
    grep -qF "zircd restarted; watching again" "$home/watch"
kill -0 "$watching" 2>/dev/null || fail "zirc watch stopped when zircd restarted: $(cat "$home/watch")"
kill "$watching"
unset ZIRC_UPDATE_EVERY

# Turned off: zircd leaves the binaries alone.
setup off "auto_update = false"
ZIRC_UPDATE_EVERY=1
zirc "$bin/zirc" daemon start || fail "zircd didn't start: $(cat "$out")"
sleep 3
cmp -s "$bin/zirc" "$real/zirc" || fail "updated with auto_update = false"
[ ! -e "$zirc_home/update.json" ] || fail "checked with auto_update = false"
unset ZIRC_UPDATE_EVERY

# A release that doesn't match its checksum isn't installed, and zirc's commands say so.
setup corrupt
fake 98.0.0
printf '98.0.0\n' >"$releases/latest.txt"
printf '%064d  zirc-98.0.0-%s.tar.gz\n' 0 "$target" >"$releases/v98.0.0/zirc-98.0.0-$target.tar.gz.sha256"
sign "$releases/v98.0.0/zirc-98.0.0-$target.tar.gz.sha256"
ZIRC_UPDATE_EVERY=1
zirc "$bin/zirc" daemon start || fail "zircd didn't start: $(cat "$out")"
eventually "zircd didn't record its failed update" test -s "$zirc_home/update.json"
cmp -s "$bin/zirc" "$real/zirc" || fail "installed a release that doesn't match its checksum"
zirc "$bin/zirc" status || fail "status failed: $(cat "$out")"
expect "zirc 98.0.0 is out, but zircd couldn't install it"
expect "doesn't match its SHA-256"
unset ZIRC_UPDATE_EVERY
zirc "$bin/zirc" update && fail "zirc update installed it anyway"
expect "doesn't match its SHA-256"

# A release without a signature isn't installed, by zircd or zirc update, and zirc's
# commands say why.
setup unsigned
fake 97.0.0
printf '97.0.0\n' >"$releases/latest.txt"
rm "$releases/v97.0.0/zirc-97.0.0-$target.tar.gz.sha256.sig"
ZIRC_UPDATE_EVERY=1
zirc "$bin/zirc" daemon start || fail "zircd didn't start: $(cat "$out")"
eventually "zircd didn't record its failed update" test -s "$zirc_home/update.json"
cmp -s "$bin/zirc" "$real/zirc" || fail "installed a release without a signature"
zirc "$bin/zirc" status || fail "status failed: $(cat "$out")"
expect "zirc 97.0.0 is out, but zircd couldn't install it"
expect "has no signature"
unset ZIRC_UPDATE_EVERY
# Stopped, so it doesn't install the release once it's signed, below, before zirc update does.
zirc "$bin/zirc" daemon stop || fail "zircd didn't stop: $(cat "$out")"
zirc "$bin/zirc" update && fail "zirc update installed an unsigned release"
expect "has no signature"
expect "installs only releases signed with its release key"

# Nor one signed with a key zirc doesn't trust.
sign "$releases/v97.0.0/zirc-97.0.0-$target.tar.gz.sha256" "$tmp/other"
zirc "$bin/zirc" update && fail "zirc update installed a release signed with another key"
expect "isn't the release key's"
cmp -s "$bin/zirc" "$real/zirc" || fail "installed a release signed with another key"

# Signed, it's installed.
sign "$releases/v97.0.0/zirc-97.0.0-$target.tar.gz.sha256"
zirc "$bin/zirc" update || fail "update failed: $(cat "$out")"
expect "updated zirc $version → 97.0.0"

# Only over HTTPS: an http:// receipt is refused, and so is file:// without a test key.
setup insecure "auto_update = false"
sed 's|file://|http://|' "$bin/.zirc-install.toml" >"$bin/receipt" && mv "$bin/receipt" "$bin/.zirc-install.toml"
zirc "$bin/zirc" update && fail "updated over http://"
expect "updates only from https:// URLs"
setup untrusted "auto_update = false"
saved_key="$test_key"
test_key=""
zirc "$bin/zirc" update && fail "updated from file:// without a test key"
expect "updates only from https:// URLs"
test_key="$saved_key"
cmp -s "$bin/zirc" "$real/zirc" || fail "updated from file:// without a test key"

# Not installed by the installer: zirc update says what to do instead.
home="$tmp/elsewhere"
zirc_home="$homes/elsewhere"
out="$home/out"
mkdir -p "$home" "$zirc_home"
zirc "$real/zirc" update && fail "updated a zirc the installer didn't install"
expect "wasn't installed by zirc's installer"
expect "curl -fsSL https://zirc.dev/install | sh"

# Somewhere this account can't write. (Root can write anywhere.)
if [ "$(id -u)" != 0 ]; then
    setup read-only
    chmod 555 "$bin"
    zirc "$bin/zirc" update && fail "updated somewhere it can't write"
    expect "can't write to"
    chmod 755 "$bin"
fi

printf 'PASS\n'
