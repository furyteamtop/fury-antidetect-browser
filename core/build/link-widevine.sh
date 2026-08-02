#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

# Populate the Widevine CDM from the Chrome already installed on this machine.
#
#   core/build/link-widevine.sh [path-to-Chrome.app]
#
# Why this is a script and not a checked-in file: the CDM is a proprietary blob.
# Using the copy already licensed onto a developer's machine is one thing;
# redistributing it in our repository or installer is another, and needs a
# licence from Google. See docs/10.
#
# Everything under core/src is gitignored, so nothing this writes can be
# committed by accident.
set -euo pipefail

CORE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC_TREE="$CORE_DIR/src"
CHROME="${1:-/Applications/Google Chrome.app}"

[ -d "$SRC_TREE" ] || { echo "!! No Chromium tree. Run fetch.sh first." >&2; exit 1; }
[ -d "$CHROME" ] || {
  echo "!! Chrome not found at $CHROME" >&2
  echo "!! Without it the build has no CDM and requestMediaKeySystemAccess" >&2
  echo "!! ('com.widevine.alpha') will be refused, which real Chrome accepts." >&2
  exit 1
}

# Match the CDM to the Chromium we are building. A mismatched CDM either fails
# to load or reports a version that contradicts the browser's own.
PINNED="$(cat "$CORE_DIR/CHROMIUM_VERSION" 2>/dev/null || echo unknown)"
FW="$CHROME/Contents/Frameworks/Google Chrome Framework.framework/Versions"

CDM_DIR="$FW/$PINNED/Libraries/WidevineCdm"
if [ ! -d "$CDM_DIR" ]; then
  echo "!! Chrome has no CDM for $PINNED. Installed versions:" >&2
  ls "$FW" 2>/dev/null | sed 's/^/     /' >&2
  echo "!! Update Chrome to match, or rebuild against a version it has." >&2
  exit 1
fi

case "$(uname -m)" in
  arm64) ARCH=arm64; PLATFORM=mac_arm64 ;;
  x86_64) ARCH=x64; PLATFORM=mac_x64 ;;
  *) echo "!! Unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac

DEST="$SRC_TREE/third_party/widevine/cdm/mac/$ARCH"
mkdir -p "$DEST"

cp "$CDM_DIR/_platform_specific/$PLATFORM/libwidevinecdm.dylib" "$DEST/"
cp "$CDM_DIR/_platform_specific/$PLATFORM/libwidevinecdm.dylib.sig" "$DEST/" 2>/dev/null || true
cp "$CDM_DIR/manifest.json" "$DEST/"
cp "$CDM_DIR/LICENSE" "$DEST/" 2>/dev/null || true

echo "==> CDM $PINNED ($ARCH) staged in third_party/widevine/cdm/mac/$ARCH"
ls -la "$DEST" | tail -n +2 | awk '{printf "    %-32s %s\n", $9, $5}'
echo
echo "Now set bundle_widevine_cdm = true in the GN args and rebuild."
