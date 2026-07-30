#!/usr/bin/env bash
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
OUT="out/$TARGET"

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

echo "==> gn gen $OUT"
(cd "$SRC" && gn gen "$OUT")

echo "==> autoninja chrome"
(cd "$SRC" && autoninja -C "$OUT" chrome)

echo "==> Built: $SRC/$OUT"

if [ "$TARGET" = "macos-arm64" ] && [ -d "$SRC/out/macos-x64" ]; then
  cat <<EOF

Both macOS slices present. Make the universal binary with:
  lipo -create -output Fury \\
    "$SRC/out/macos-arm64/Fury.app/Contents/MacOS/Fury" \\
    "$SRC/out/macos-x64/Fury.app/Contents/MacOS/Fury"
EOF
fi
