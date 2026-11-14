#!/bin/sh
# Release archives carry no extended attributes: macOS's tar would store them as pax headers
# (LIBARCHIVE.xattr.*, SCHILY.xattr.*), which GNU tar warns about on every Linux install.
# Checks package.sh on files given an attribute here, where the system can, and this machine's
# real release archive.
#
#   package_test.sh <package.sh> <archive> <its checksum>
# POSIX sh, as install.sh and package.sh are: no pipefail, so gzip -t checks the archive first.
set -eu

package=$1 dist=""
shift
for file in "$@"; do
    case $file in *.tar.gz) dist=$file ;; esac
done
[ -n "$dist" ] || { echo "FAIL: no .tar.gz among $*" >&2; exit 1; }
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# clean <archive>: no pax or GNU extended headers, and no AppleDouble ._ files.
clean() {
    gzip -t "$1"
    found=$(gzip -dc "$1" | LC_ALL=C grep -a -o -E 'LIBARCHIVE\.[a-z.]+|SCHILY\.[a-z.]+|[0-9]+ path=' | sort -u || true)
    if [ -n "$found" ]; then
        echo "FAIL: $1 has extended headers: $found" >&2
        exit 1
    fi
    if tar -tzf "$1" | grep -q '/\._'; then
        echo "FAIL: $1 has AppleDouble ._ files" >&2
        exit 1
    fi
}

mkdir "$tmp/in"
printf 'binary\n' >"$tmp/in/zirc"
printf 'licence\n' >"$tmp/in/LICENSE"
if command -v xattr >/dev/null 2>&1; then
    xattr -w com.example.zirc-test yes "$tmp/in/zirc"
elif command -v setfattr >/dev/null 2>&1; then
    setfattr -n user.zirc-test -v yes "$tmp/in/zirc" 2>/dev/null || true
fi
"$package" "$tmp/zirc-0.0.0-test.tar.gz" "$tmp/zirc-0.0.0-test.tar.gz.sha256" "$tmp/in/zirc" -- "$tmp/in/LICENSE"
clean "$tmp/zirc-0.0.0-test.tar.gz"
[ "$(tar -tzf "$tmp/zirc-0.0.0-test.tar.gz" | sort | tr '\n' ' ')" = "zirc-0.0.0-test/ zirc-0.0.0-test/LICENSE zirc-0.0.0-test/zirc " ] || {
    echo "FAIL: unexpected entries: $(tar -tzf "$tmp/zirc-0.0.0-test.tar.gz")" >&2
    exit 1
}

clean "$dist"
echo "PASS: no extended headers in $(basename "$dist")"
