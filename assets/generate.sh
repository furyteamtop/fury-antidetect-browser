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

# Compared as numbers. The first version pasted the two values together with a
# multiplication sign and matched the result against a literal, which failed on
# a correct 1024x1024 file because the separator did not survive the round trip.
W=$(sips -g pixelWidth "$SRC" | awk '/pixelWidth/{print $2}')
H=$(sips -g pixelHeight "$SRC" | awk '/pixelHeight/{print $2}')
if [ "$W" != "1024" ] || [ "$H" != "1024" ]; then
  echo "!! assets/icon.png is ${W}x${H}; it must be 1024x1024." >&2
  echo "!! macOS builds its whole icon set down from that size." >&2
  exit 1
fi

# An icon with no alpha is a black tile in the dock. The source art draws its own
# rounded square, so the surround must be transparent and not merely dark.
if [ "$(sips -g hasAlpha "$SRC" | awk '/hasAlpha/{print $2}')" != "yes" ]; then
  echo "!! assets/icon.png has no alpha channel." >&2
  echo "!! The area outside the rounded square must be transparent, or macOS" >&2
  echo "!! shows a black square in the dock." >&2
  exit 1
fi

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
# AND where the Tauri bundler reads it. tauri.conf.json lists
# icons/icon.icns, and the first version of this script wrote the .icns only to
# core/branding — so the shell kept shipping the icon it was built with and the
# regenerated one went somewhere nothing looked. Visible immediately, and
# invisible in a diff.
cp "$BRAND/app.icns" "$TAURI/icon.icns"
rm -rf "$(dirname "$ICONSET")"

echo "==> Windows .ico"
python3 - "$SRC" "$TAURI/icon.ico" <<'ICO'
import sys
from PIL import Image
src, out = sys.argv[1], sys.argv[2]
im = Image.open(src).convert("RGBA")
# A .ico is a container; Windows picks the size it wants. 256 is the largest it
# reads and 16 is the smallest it shows.
im.save(out, sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
ICO

echo "==> social preview"
# The card GitHub shows when the repository is linked anywhere — Telegram,
# Twitter, Slack, a search result.
#
# Not urgent and not a fix: with none set, GitHub generates its own card from
# the name, the description and the avatar, so the fallback is respectable
# rather than blank. This is polish.
#
# 1280x640 exactly, because GitHub crops to 2:1 and the sources are 1920x819
# (2.34:1). Uploading a source directly loses a slice off the top and bottom.
#
# The dark wordmark on the brand's near-black, rather than the light one: every
# client that shows this puts it on its own background, some white and some
# black, so the card carries its own and stops depending on where it lands.
python3 - "$HERE/logo-dark.png" "$HERE/social-preview.png" <<'SOCIAL'
import sys
from PIL import Image

src, out = sys.argv[1], sys.argv[2]
W, H = 1280, 640
BG = (13, 14, 18)          # sampled from icon.png's own background

logo = Image.open(src).convert("RGBA")
# Crop to the ink. The source carries transparent margins of its own, and
# centring the FILE rather than the ARTWORK puts the wordmark visibly high.
logo = logo.crop(logo.getbbox())

# 68% of the width. Wide enough to read in a small card, with enough air that
# it does not look cropped when a client rounds the corners.
target_w = int(W * 0.68)
scale = target_w / logo.width
logo = logo.resize((target_w, max(1, int(logo.height * scale))), Image.LANCZOS)

card = Image.new("RGB", (W, H), BG)
card.paste(logo, ((W - logo.width) // 2, (H - logo.height) // 2), logo)
card.save(out, optimize=True)
print(f"    {out}  {W}x{H}")
SOCIAL

echo "==> Done."
echo "    desktop/src-tauri/icons/   Tauri shell"
echo "    desktop/src-tauri/icons/icon.icns  and icon.ico"
echo "    core/branding/app.icns     patch 0900, when it is written"
echo "    assets/social-preview.png  repository Settings -> General -> Social preview"
