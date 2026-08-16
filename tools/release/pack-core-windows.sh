#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# Pack the Windows core into the shape install-core expects.
#
#     tools/release/pack-core-windows.sh        # from Git Bash, on Windows
#
# package.sh cannot do this and the reason is structural rather than an
# oversight: its core path runs "$app/Contents/MacOS/Fury" --version to prove the
# bundle starts, and a Windows core is a directory of loose files with no bundle
# and no console output. Rather than teach that function two shapes, this is the
# Windows half, and package.sh stays the thing that assembles a release from
# artifacts that already exist.
#
# An explicit list rather than the whole output directory: that directory is
# 11 GB of build tree, almost all of it test binaries and .runtime_deps files.
# What a browser needs to run is under 300 MB of it.
#
# Deliberately excluded, and worth naming so nobody adds them back by reflex:
#   dbgcore.dll, dbghelp.dll, msdia140.dll, symsrv.dll -- debugging tools, used
#     by the build and by crash symbolisation, not by the browser.
#   VkICD_mock_icd.dll, VkLayer_khronos_validation.dll -- a fake Vulkan driver
#     and a validation layer. Shipping a mock ICD to a user is shipping a GPU
#     that does not exist, which is the opposite of what this project is for.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
out="${OUT_DIR:-$here/core/src/out/windows-x64-first.noindex}"
dist="${DIST:-$here/dist}"
cd "$out"

files=(
  chrome.exe chrome_proxy.exe chrome_pwa_launcher.exe notification_helper.exe
  chrome.dll chrome_elf.dll chrome_wer.dll
  d3dcompiler_47.dll dxcompiler.dll dxil.dll
  libEGL.dll libGLESv2.dll vk_swiftshader.dll vulkan-1.dll vk_swiftshader_icd.json
  eventlog_provider.dll
  msvcp140.dll msvcp140_atomic_wait.dll vccorlib140.dll vcruntime140.dll vcruntime140_1.dll
  chrome_100_percent.pak chrome_200_percent.pak resources.pak
  icudtl.dat snapshot_blob.bin v8_context_snapshot.bin
)
# The directory list comes from chrome/installer/mini_installer/chrome.release,
# Chromium's own manifest of what ships, rather than from guessing. Two of these
# were missing from the first attempt and the staged core would not start.
dirs=(locales resources MEIPreload angledata PrivacySandboxAttestationsPreloaded IwaKeyDistribution)

# The version manifest. chrome.release lists it as `*.*.*.*.manifest` and its
# absence is not a missing feature: chrome.exe embeds a side-by-side reference to
# the assembly this file describes, so without it Windows refuses to start the
# process at all, with
#
#     side-by-side configuration is incorrect
#
# which names no file and sends you to sxstrace. Measured 16.08.2026 -- it cost
# one packing round and was found by a probe that looked like it had hung.
manifest="$(ls -1 *.*.*.*.manifest 2>/dev/null | head -1)"
[ -n "$manifest" ] || { echo "!! no <version>.manifest in the build output" >&2; exit 1; }
files+=("$manifest")

missing=0
for f in "${files[@]}"; do
  [ -f "$f" ] || { echo "!! missing: $f" >&2; missing=1; }
done
[ "$missing" = 0 ] || exit 1

# Staged under a directory named "Fury" so the archive expands to one folder
# with a name, the same shape the macOS tarball has.
rm -rf "$here/dist-core" && mkdir -p "$here/dist-core/Fury"
cp "${files[@]}" "$here/dist-core/Fury/"
for d in "${dirs[@]}"; do [ -d "$d" ] && cp -r "$d" "$here/dist-core/Fury/"; done

echo "== staged"
du -sh "$here/dist-core/Fury" | cut -f1

# No --version check here. chrome.exe is a GUI-subsystem binary: it writes
# nothing to a console and its exit code says nothing useful, so "it did not
# start" from that is a false alarm. Whether the staged set is complete is
# answered by rendering a page -- tools/pack-core checks that separately.

echo "== packing"
mkdir -p "$dist"
tar -cJf "$dist/fury-core-windows.tar.xz" -C "$here/dist-core" Fury
ls -la "$dist/fury-core-windows.tar.xz" | awk '{printf "   %.0f MB\n", $5/1000000}'
