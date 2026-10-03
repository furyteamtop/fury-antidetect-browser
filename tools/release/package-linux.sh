#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# Build the Linux release: shell bundles plus the matching core archive.
#
#     tools/release/package-linux.sh [version]     # on Linux x86_64
#
# Needs FURY_CORE_DIR: a directory with the unpacked Linux core (chrome inside,
# as packed by tools/release/pack-core-linux.sh). The shell ships without the
# browser — same split as macOS and Windows, 12 MB shell, core on its own
# schedule — but a release whose core does not exist cannot be matched against
# the core it was tested with, so packaging refuses to run without one.
#
# Asset names are the stable contract, infix linux-x64:
#
#     fury-<version>-linux-x64.AppImage
#     fury-<version>-linux-x64.deb
#     fury-core-<version>-linux-x64.tar.xz
#
# agent/src/core_download.rs looks releases up with the same infix; a rename
# here is a "no core asset in the latest release" there. tools/ci/
# check-linux-release.sh pins this agreement on every commit.
#
# No checksums are written. The release flow that publishes them is a separate
# decision — see docs/linux.md.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
version="${1:-}"
core="${FURY_CORE_DIR:-}"
out="${OUT:-$here/dist}"

if [ -z "$version" ]; then
	version=$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f&&/^version/{gsub(/[^0-9.]/,"");print;exit}' "$here/Cargo.toml")
fi
if ! printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'; then
	echo "!! expected version MAJOR.MINOR.PATCH[-suffix], got: $version" >&2
	exit 2
fi
if [ "$(uname -s)" != Linux ] || [ "$(uname -m)" != x86_64 ]; then
	echo '!! Linux packaging currently supports x86_64 hosts only' >&2
	exit 1
fi

[ -n "$core" ] || {
	echo '!! FURY_CORE_DIR is required: a Linux release must include its matching Chromium core' >&2
	exit 1
}
[ -d "$core" ] || {
	echo "!! no core directory: $core" >&2
	exit 1
}
binary="$core/chrome"
[ -x "$binary" ] || binary="$core/fury-core"
[ -x "$binary" ] || {
	echo "!! core directory contains no executable chrome or fury-core: $core" >&2
	exit 1
}
"$binary" --version

cd "$here/desktop"
npm ci
# APPIMAGE_EXTRACT_AND_RUN lets the AppImage packaging tools run on a build box
# without FUSE (a CI runner mounts no /dev/fuse): linuxdeploy and appimagetool
# then unpack and repack instead of mounting.
APPIMAGE_EXTRACT_AND_RUN=1 npx tauri build --bundles appimage,deb
"$here/tools/release/patch-appimage-wayland.sh" "${here}"/target/release/bundle/appimage/*.AppImage

mkdir -p "$out"
appimage=("$here"/target/release/bundle/appimage/*.AppImage)
deb=("$here"/target/release/bundle/deb/*.deb)
[ -f "${appimage[0]}" ] || {
	echo '!! AppImage was not produced' >&2
	exit 1
}
[ -f "${deb[0]}" ] || {
	echo '!! deb was not produced' >&2
	exit 1
}

# The deb's Version field comes from the manifests tauri reads
# (desktop/package.json), not from $version: renaming the artifact cannot
# change what dpkg records, and core_download.rs matches releases by artifact
# name — a name that disagrees with the embedded version would offer a
# package whose metadata says something else. Reject the mismatch here,
# before anything is copied, instead of shipping it.
deb_name="${deb[0]##*/}"; deb_name="${deb_name%.deb}"
embedded="$(printf '%s' "$deb_name" | cut -d_ -f2)"
if [ "$embedded" != "$version" ]; then
	echo "!! version mismatch: artifacts are named $version, but the deb carries $embedded." >&2
	echo "!! The package version comes from desktop/package.json; align it with the" >&2
	echo "!! requested version (or pass the manifests' version) and rebuild." >&2
	exit 2
fi

cp "${appimage[0]}" "$out/fury-$version-linux-x64.AppImage"
cp "${deb[0]}" "$out/fury-$version-linux-x64.deb"

# Flat archive, chrome at the top: the consumer's core_binary() joins its
# leaf names directly onto the install directory — see pack-core-linux.sh.
tar -cJf "$out/fury-core-$version-linux-x64.tar.xz" -C "$core" .

echo "== packed"
ls -la "$out" | grep "fury-.*$version"
