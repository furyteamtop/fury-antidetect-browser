#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

# Generate every derived icon size from assets/icon.png.
#
# One source, many outputs, and none of the outputs are edited by hand — an icon
# set where one size was touched separately is how an app ends up with a
# different logo in the dock and in the about box.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
SRC="$HERE/icon.png"

[ -f "$SRC" ] || { echo "!! assets/icon.png not found. See assets/README.md." >&2; exit 1; }

DIMS=$(sips -g pixelWidth -g pixelHeight "$SRC" | awk '/pixel/{print $2}' | paste -sd× -)
case "$DIMS" in
  1024×1024) ;;
  *) echo "!! assets/icon.png is $DIMS; it must be 1024×1024." >&2; exit 1 ;;
esac

TAURI="$ROOT/desktop/src-tauri/icons"
mkdir -p "$TAURI"
echo "==> Tauri shell icons"
for s in 32 64 128 256 512; do
  sips -z $s $s "$SRC" --out "$TAURI/${s}x${s}.png" >/dev/null
done
cp "$TAURI/256x256.png" "$TAURI/128x128@2x.png"
sips -z 107 107 "$SRC" --out "$TAURI/Square107x107Logo.png" >/dev/null
sips -z 142 142 "$SRC" --out "$TAURI/Square142x142Logo.png" >/dev/null
cp "$SRC" "$TAURI/icon.png"

echo "==> macOS .icns"
ICONSET="$(mktemp -d)/Fury.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s "$SRC" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s*2)) $((s*2)) "$SRC" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
BRAND="$ROOT/core/branding"
mkdir -p "$BRAND"
iconutil -c icns "$ICONSET" -o "$BRAND/app.icns"
cp "$SRC" "$BRAND/icon-1024.png"
rm -rf "$(dirname "$ICONSET")"

echo "==> Done."
echo "    desktop/src-tauri/icons/   Tauri shell"
echo "    core/branding/app.icns     patch 0900, when it is written"
