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

case "$TARGET" in
  macos-arm64|macos-x64)
    [ "$(uname -s)" = "Darwin" ] || {
      echo "!! macOS targets require a physical Mac. There is no cross-compile." >&2
      exit 1
    }
    ;;
  windows-x64)
    case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) ;; *)
      echo "!! Build Windows on Windows. Cross-building is possible but brittle." >&2
      exit 1 ;;
    esac
    ;;
  *) echo "!! Unknown target: $TARGET" >&2; exit 1 ;;
esac

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
