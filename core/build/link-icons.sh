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

# The step that was missing, and it is the one that decides what people see.
#
# Assets.car is checked into the Chromium tree ALREADY COMPILED — chrome/BUILD.gn
# lists it as a source and copies it, it is not built from the .xcassets beside
# it. So replacing the PNGs above did nothing to the file the application
# actually reads, and since Info.plist carries CFBundleIconName = "AppIcon",
# macOS prefers that catalogue over app.icns. The result was a browser called
# Fury, with Fury's app.icns, showing Chromium's blue icon everywhere it
# counted: the Dock, Spotlight, the Applications list.
#
# Compiling the catalogue ourselves is what makes the replaced images reach it.
echo "==> compiling Assets.car"
CAR_TMP="$(mktemp -d)"
trap 'rm -rf "$CAR_TMP"' EXIT
if xcrun actool --compile "$CAR_TMP" --app-icon AppIcon \
        --output-partial-info-plist "$CAR_TMP/partial.plist" \
        --platform macosx --minimum-deployment-target 10.15 \
        "$THEME/Assets.xcassets" > "$CAR_TMP/actool.log" 2>&1 \
   && [ -s "$CAR_TMP/Assets.car" ]; then
    cp "$CAR_TMP/Assets.car" "$THEME/Assets.car"
    echo "    $(wc -c < "$THEME/Assets.car" | tr -d ' ') bytes"
else
    # Not fatal: without Xcode there is no actool, and a build with the stock
    # catalogue still works — it just wears the wrong icon. Saying which is the
    # difference between a known cosmetic gap and a mystery.
    echo "!! actool did not produce a catalogue; the icon will stay Chromium's." >&2
    echo "   Needs Xcode (not just the command line tools). Log:" >&2
    tail -5 "$CAR_TMP/actool.log" >&2 || true
fi

echo
echo "Done. The build now produces Fury.app with Fury's icons."
echo "ninja does NOT delete the old Chromium.app when the output name changes —"
echo "both will sit in the output directory, and anyone testing the old path"
echo "will report that nothing happened."
