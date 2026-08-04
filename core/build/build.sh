#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

# Build the patched Chromium.
#
# Usage: core/build/build.sh <target>
#   targets: macos-arm64 | macos-x64 | windows-x64
#
# Full build: 1.5-3 h on 32+ cores, 4-8 h on a laptop. Incremental after one
# patch: 5-30 min. Do not delete out/ between runs — that is your ccache.
set -euo pipefail

CORE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$CORE_DIR/src"
TARGET="${1:?usage: build.sh <macos-arm64|macos-x64|windows-x64>}"
# `.noindex` is not decoration: it is the only thing measured to work.
#
# ninja writes the five helper applications as standalone bundles beside
# Fury.app, so a machine that builds ends up with "Fury Helper", "Fury Helper
# (GPU)" and two things called "Fury" in Spotlight, none of which anybody can
# usefully open. A directory whose name ends in .noindex is excluded from the
# index — the mechanism Xcode uses for DerivedData — and it was checked the same
# way everything else here is: two identical bundles, one in a plain directory
# and one in a .noindex, and only the plain one came back from mdfind.
#
# `.metadata_never_index`, which is the documented answer and the obvious first
# try, does NOT work on macOS 26. Both bundles were indexed. It is written below
# anyway because it costs nothing and older systems honour it.
#
# Renaming an existing output directory is cheap: ninja's paths inside it are
# relative, so out/macos-arm64-lowmem -> out/macos-arm64-lowmem.noindex cost 11
# steps and 22 seconds on a tree that takes six hours to build from scratch.
OUT="out/$TARGET.noindex"

# An output directory from before this convention. Renamed rather than left, so
# the six hours already spent are not spent again.
if [ -d "$SRC/out/$TARGET" ] && [ ! -d "$SRC/$OUT" ]; then
  echo "==> moving out/$TARGET to $OUT so Spotlight stops indexing it"
  mv "$SRC/out/$TARGET" "$SRC/$OUT"
fi

export PATH="$CORE_DIR/depot_tools:$PATH"
export DEPOT_TOOLS_UPDATE=0
DEPOT_TOOLS="$CORE_DIR/depot_tools"

# --- one-time depot_tools bootstrap -----------------------------------------
# gn and friends need python3_bin_reldir.txt, which only appears after a
# bootstrap. DEPOT_TOOLS_UPDATE=0 (kept, for reproducible builds) suppresses the
# implicit one, so run it explicitly — and run it with cwd INSIDE depot_tools,
# because its scripts resolve relative paths from the working directory, not
# from their own location. Invoking it as ./depot_tools/ensure_bootstrap fails
# with a confusing "cipd_client_version.digests: No such file" instead.
if [ ! -f "$DEPOT_TOOLS/python3_bin_reldir.txt" ]; then
  echo "==> Bootstrapping depot_tools (one time)"
  (cd "$DEPOT_TOOLS" && ./ensure_bootstrap)
fi

# Prefix match, so variants like macos-arm64-lowmem resolve to the right platform.
case "$TARGET" in
  macos-arm64*|macos-x64*)
    [ "$(uname -s)" = "Darwin" ] || {
      echo "!! macOS targets require a physical Mac. There is no cross-compile." >&2
      exit 1
    }
    ;;
  windows-x64*)
    case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) ;; *)
      echo "!! Build Windows on Windows. Cross-building is possible but brittle." >&2
      exit 1 ;;
    esac
    ;;
  *) echo "!! Unknown target: $TARGET" >&2; exit 1 ;;
esac

# A 16 GB machine cannot survive an official (ThinLTO) build. Say so before
# burning four hours, not after the linker gets OOM-killed.
if [ "$(uname -s)" = "Darwin" ]; then
  ram_gb=$(( $(sysctl -n hw.memsize) / 1073741824 ))
  if [ "$ram_gb" -lt 24 ] && [ "${TARGET#*lowmem}" = "$TARGET" ]; then
    echo "!! This machine has ${ram_gb} GB RAM. An official build enables ThinLTO," >&2
    echo "!! and a single LTO link can hold 8-16 GB. Use the low-memory config:" >&2
    echo "!!   $0 ${TARGET}-lowmem" >&2
    echo "!! Override with FORCE=1 if you know what you are doing." >&2
    [ "${FORCE:-0}" = "1" ] || exit 1
  fi
fi

ARGS_FILE="$CORE_DIR/args/$TARGET.gn"
[ -f "$ARGS_FILE" ] || { echo "!! Missing $ARGS_FILE" >&2; exit 1; }

mkdir -p "$SRC/$OUT"
cp "$ARGS_FILE" "$SRC/$OUT/args.gn"

# Ask Spotlight to skip the build directory.
#
# ninja writes the five helper applications as standalone bundles beside
# Fury.app before copying them into the framework, so a machine that builds ends
# up with "Fury Helper", "Fury Helper (GPU)" and two things called "Fury" in
# search — none of which anybody can usefully open.
#
# .metadata_never_index is the documented way to ask, and on macOS 26 it does
# NOT work for an ordinary directory: two identical bundles, one with the marker
# and one without, were both indexed. Written anyway because it costs nothing
# and older systems honour it, but it is not the fix and this says so.
#
# What does work is a package extension — Spotlight indexes a bundle as one item
# and does not look inside — which is why real Chrome shows a single result
# despite carrying five helpers, and why an installed core now lives in
# core.bundle (agent/src/paths.rs). ninja's output layout is not ours to
# rename, so on a build machine the honest answer is System Settings →
# Spotlight → Privacy, with core/src/out added to it.
if [ ! -f "$SRC/out/.metadata_never_index" ]; then
  : > "$SRC/out/.metadata_never_index"
fi

echo "==> gn gen $OUT"
(cd "$SRC" && gn gen "$OUT")

# J caps parallelism, which is the only way to actually bound CPU use. `nice`
# alone just yields under contention — an idle machine still gets fully consumed,
# which is not what someone who asked for "20%" wants. On a 10-core machine J=2
# is ~20%.
JOBS_ARG=""
if [ -n "${J:-}" ]; then
  JOBS_ARG="-j$J"
  echo "==> limiting to $J parallel jobs"
fi

echo "==> autoninja chrome"
(cd "$SRC" && autoninja -C "$OUT" $JOBS_ARG chrome)

echo "==> Built: $SRC/$OUT"

# Say it where it happens, not only in a design document nobody reads while a
# build is running.
#
# link-widevine.sh already warns that the CDM is proprietary and that
# redistributing it needs a licence from Google. What nothing said until now is
# that the bundle sitting in the output directory CONTAINS it — 20 MB of
# unredistributable binary, staged there by the build that just finished,
# because the args asked for it.
if grep -q '^ *bundle_widevine_cdm *= *true' "$SRC/$OUT/args.gn" 2>/dev/null; then
  CDM="$SRC/$OUT"/*.app/Contents/Frameworks/*.framework/Versions/*/Libraries/WidevineCdm
  if compgen -G "$CDM" > /dev/null 2>&1; then
    cat >&2 <<EOF

!! This bundle contains the Widevine CDM.

   $(du -h $CDM 2>/dev/null | tail -1 | cut -f1) of proprietary binary, staged out of the Google Chrome installed on
   this machine. Using a copy already licensed onto your own machine is one
   thing; passing the bundle to anyone else is redistribution, and that needs a
   licence from Google.

   Do not upload, publish or hand on this .app. For a build you can give away,
   use core/args/macos-arm64.gn — and read the note in it first, because a
   build without Widevine answers requestMediaKeySystemAccess differently from
   real Chrome, which is its own problem.

   See NOTICE section 5 and docs/10-legal-licensing.md.
EOF
  fi
fi

if [ "$TARGET" = "macos-arm64" ] && [ -d "$SRC/out/macos-x64.noindex" ]; then
  cat <<EOF

Both macOS slices present. Merge them with Chromium's own universalizer:

  python3 "$SRC/chrome/installer/mac/universalizer.py" \\
    "$SRC/out/macos-arm64.noindex/Fury.app" \\
    "$SRC/out/macos-x64.noindex/Fury.app" \\
    "$SRC/out/macos-universal.noindex/Fury.app"

NOT lipo on the main executable. That advice used to be here and it is wrong
for this bundle: lipo would merge one 76 KB launcher and leave the 540 MB
framework, five helper applications and every dylib single-architecture. The
result runs on the machine it was built for and, on the other one, fails to
load its own framework — a bundle that looks universal and is not.

universalizer.py walks both trees and merges every Mach-O file it finds, which
is why Chromium ships it.

EOF
fi
