#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# Pack the Linux core into the shape install-core expects.
#
#     tools/release/pack-core-linux.sh        # on Linux, after build.sh linux-x64
#
# The Windows twin is pack-core-windows.sh, and the reason it exists there
# holds here too: package.sh's core path proves the bundle by running
# "$app/Contents/MacOS/Fury" --version, which is an .app-bundle path. A Linux
# core is a directory of loose files, so it gets this script instead and
# package.sh stays the thing that assembles a release from artifacts that
# already exist.
#
# An explicit list rather than the whole output directory, same as Windows:
# out/linux-x64.noindex is gigabytes of build tree, and what a browser needs
# to run is a few hundred megabytes of it.
#
# The archive is FLAT — chrome at the top, no wrapping directory. That is the
# shape the consumer looks for: agent/src/main.rs's core_leaves() joins each
# name DIRECTLY onto core.bundle, with no one-level-down walk, so an archive
# with a top-level Fury/ directory installs ("no core found" not firing) and
# then never launches ("no core binary" instead). install_core::find_core
# tolerates both shapes; core_binary does not, and the flat archive is the
# one that works for both. Differs from Windows, whose archive carries the
# Fury/ directory because its leaves name it.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
out="${OUT_DIR:-$here/core/src/out/linux-x64.noindex}"
dist="${DIST:-$here/dist}"
[ -d "$out" ] || { echo "!! no build output at $out -- run core/build/build.sh linux-x64 first" >&2; exit 1; }
cd "$out"

# Refuse to pack a build that was compiled to MEASURE itself. Same check as
# pack-core-windows.sh runs on chrome.dll, on the same reasoning: a
# chrome_pgo_phase = 1 build starts, browses and ships fine and is about a
# third slower in ways JavaScript can see.
if grep -a -q '__llvm_profile' chrome 2>/dev/null; then
  echo "!! the chrome binary carries the PGO instrumentation runtime (__llvm_profile)." >&2
  echo "!! This is a chrome_pgo_phase = 1 build: it exists to record a profile," >&2
  echo "!! not to be used. Set chrome_pgo_phase = 2 in core/args/linux-x64.gn and rebuild." >&2
  exit 1
fi

# The executable and its runtime files for a component (static) release
# build. Drawn from chrome/BUILD.gn's installer targets and corrected against
# a real tree when the first Linux core is packed — the missing-file check
# below fails the job fast and names what to add, which is the mechanism the
# Windows list was corrected through.
files=(
  chrome
  chrome_crashpad_handler
  chrome_100_percent.pak chrome_200_percent.pak resources.pak
  icudtl.dat v8_context_snapshot.bin
  libEGL.so libGLESv2.so
  libvk_swiftshader.so vk_swiftshader_icd.json
)
# Chromium's own manifest of what ships (chrome/installer/linux) lists these
# directories; the pack fails loudly if a future build drops one.
dirs=(locales resources MEIPreload)

missing=0
for f in "${files[@]}"; do
  [ -f "$f" ] || { echo "!! missing: $f" >&2; missing=1; }
done
[ "$missing" = 0 ] || exit 1

# Stage into a fresh, task-unique directory and never remove a pre-existing
# one: an earlier dist-core.* on this machine may be the only copy of a core
# that took hours to build, and a packer has no business deleting it to make
# room. The staging directory is removed on the way out; the archive is the
# product. -p keeps the runtime permissions as the build produced them — a
# chrome that arrives non-executable is a core that installs and never
# launches.
stage="$(mktemp -d "$here/dist-core.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
cp -p "${files[@]}" "$stage/"
for d in "${dirs[@]}"; do [ -d "$d" ] && cp -rp "$d" "$stage/"; done
# chrome_sandbox, the setuid helper, exists only when the build produced it;
# user-namespaces distros never need it, but a build that made one clearly
# meant it to ship.
if [ -f chrome_sandbox ]; then
  cp -p chrome_sandbox "$stage/"
fi

echo "== staged"
du -sh "$stage" | cut -f1

# Unlike Windows, a Linux core can prove itself here: chrome --version prints
# and exits without a display server, so a staged set that does not start
# stops the pack instead of the user's first launch.
"$stage/chrome" --version

version="$(cat "$here/core/CHROMIUM_VERSION")"
[ -n "$version" ] || { echo "!! core/CHROMIUM_VERSION is empty" >&2; exit 1; }
mkdir -p "$dist"
tar -cJf "$dist/fury-core-$version-linux-x64.tar.xz" -C "$stage" .
ls -la "$dist/fury-core-$version-linux-x64.tar.xz" | awk '{printf "   %.0f MB\n", $5/1000000}'
