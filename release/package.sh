#!/bin/sh
# Packages a release: zirc-<version>-<platform>.tar.gz, holding zirc, zircd, zirc-remote and
# the licences in a directory of the same name, and its SHA-256 in sha256sum's format.
#
#   package.sh <archive> <checksum> <binary>... -- <licence>...
#
# Bazel runs it for //release:dist; see release/BUILD.bazel.
set -eu

archive="$1"
checksum="$2"
shift 2

name="$(basename "$archive" .tar.gz)"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/$name"
binaries=yes
for file in "$@"; do
    if [ "$file" = -- ]; then
        binaries=no
        continue
    fi
    cp "$file" "$stage/$name/"
    mode=644
    [ "$binaries" = no ] || mode=755
    chmod "$mode" "$stage/$name/$(basename "$file")"
done

# Plain ustar entries, without the files' extended attributes: macOS's bsdtar otherwise
# stores them (com.apple.provenance, quarantine) as pax headers, and GNU tar on Linux warns
# "Ignoring unknown extended header keyword 'LIBARCHIVE.xattr.…'" for each file it unpacks.
# COPYFILE_DISABLE keeps out macOS's ._ AppleDouble files too. package_test.sh checks it.
export COPYFILE_DISABLE=1
set -- --format=ustar
if tar --version 2>/dev/null | grep -q bsdtar; then
    set -- "$@" --no-mac-metadata --no-xattrs --no-acls --no-fflags
fi
# gzip -n leaves the time out of the archive's header.
tar "$@" -cf - -C "$stage" "$name" | gzip -n >"$archive"

if command -v sha256sum >/dev/null 2>&1; then
    hash="$(sha256sum "$archive" | cut -d ' ' -f 1)"
else
    hash="$(shasum -a 256 "$archive" | cut -d ' ' -f 1)"
fi
printf '%s  %s\n' "$hash" "$name.tar.gz" >"$checksum"
