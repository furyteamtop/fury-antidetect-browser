#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# Put Fury's icons into the Chromium tree before a build.
#
#   core/build/link-icons.sh
#
# Why a script and not part of patch 0900: the icons are PNGs and an .icns, and
# a quilt-style patch series carrying binaries is a series nobody can read or
# rebase. The same argument link-widevine.sh makes for the CDM, for the same
# reason — core/src is gitignored, so nothing this writes can be committed by
# accident.
#
# Run after apply.sh and before build.sh. Skipping it produces a browser called
# Fury wearing Chromium's icons, which is worse than either.
set -euo pipefail

CORE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT="$(cd "$CORE_DIR/.." && pwd)"
SRC="$CORE_DIR/src"
THEME="$SRC/chrome/app/theme/chromium/mac"

[ -d "$SRC" ] || { echo "!! No Chromium tree. Run fetch.sh first." >&2; exit 1; }
[ -f "$ROOT/assets/icon.png" ] || {
    echo "!! assets/icon.png is missing. See assets/README.md." >&2
    exit 1
}
[ -f "$ROOT/core/branding/app.icns" ] || {
    echo "==> Generating the icon set first"
    "$ROOT/assets/generate.sh"
}

echo "==> app.icns"
cp "$ROOT/core/branding/app.icns" "$THEME/app.icns"

echo "==> asset catalogue"
python3 - "$ROOT/assets/icon.png" "$THEME" <<'PY'
import sys, pathlib
from PIL import Image

src = Image.open(sys.argv[1]).convert("RGBA")
theme = pathlib.Path(sys.argv[2])

n = 0
for p in (theme / "Assets.xcassets/AppIcon.appiconset").glob("appicon_*.png"):
    size = int(p.stem.split("_")[1])
    src.resize((size, size), Image.LANCZOS).save(p)
    n += 1
for p in (theme / "Assets.xcassets/Icon.iconset").glob("icon_*.png"):
    # icon_256x256.png and icon_256x256@2x.png — the @2x is 512.
    px = 512 if "@2x" in p.stem else int(p.stem.split("_")[1].split("x")[0])
    src.resize((px, px), Image.LANCZOS).save(p)
    n += 1
print(f"    {n} images replaced")
PY

echo
echo "Done. The build now produces Fury.app with Fury's icons."
echo "ninja does NOT delete the old Chromium.app when the output name changes —"
echo "both will sit in the output directory, and anyone testing the old path"
echo "will report that nothing happened."
