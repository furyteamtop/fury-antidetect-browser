#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# The signed in-app update for a release: what the application's "Update"
# button downloads (desktop/src-tauri/src/update.rs).
#
#     tools/release/updater-feed.sh <version> <stapled Fury.app> <setup.exe> <out dir>
#
# Writes into <out dir>:
#   fury-<v>-macos-arm64.app.tar.gz      the notarised, stapled application
#   fury-<v>-macos-arm64.app.tar.gz.sig
#   fury-<v>-windows-x64-setup.exe.sig   for the installer already in <out dir>
#   latest.json                          what the updater reads, from
#                                        releases/latest/download/latest.json
#
# Signed here, on the release Mac, with ~/.private_keys/fury-updater.key
# (FURY_UPDATER_KEY to override). The Windows installer is built on the box and
# signed after it comes back, so the private key never leaves this machine.
# Every installed copy checks these signatures against the public key in
# tauri.conf.json: lose the private key and no installed copy can be updated
# again; leak it and anybody who can publish a release can update them all.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

version="${1:?usage: updater-feed.sh <version> <stapled Fury.app> <setup.exe> <out dir>}"
app="${2:?the stapled Fury.app}"
setup="${3:?the Windows setup.exe}"
out="$(cd "${4:?the release directory}" && pwd)"
key="${FURY_UPDATER_KEY:-$HOME/.private_keys/fury-updater.key}"
repo="https://github.com/furyteamtop/fury-antidetect-browser/releases/download/v$version"

[ -d "$app" ] || { echo "!! no application at $app" >&2; exit 1; }
[ -f "$setup" ] || { echo "!! no installer at $setup" >&2; exit 1; }
[ -f "$key" ] || { echo "!! no updater key at $key" >&2; exit 1; }

# The archive must hold the bundle exactly as notarised: a stapled ticket the
# updater does not carry is a Gatekeeper check over the network on first open.
stapler validate "$app" >/dev/null || { echo "!! $app is not stapled" >&2; exit 1; }
plist_version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")
[ "$plist_version" = "$version" ] || { echo "!! $app is $plist_version, not $version" >&2; exit 1; }

mac="fury-$version-macos-arm64.app.tar.gz"
win="$(basename "$setup")"
[ "$win" = "fury-$version-windows-x64-setup.exe" ] || { echo "!! installer named $win" >&2; exit 1; }
[ "$(cd "$(dirname "$setup")" && pwd)" = "$(cd "$out" && pwd)" ] || cp "$setup" "$out/$win"

# No AppleDouble files: macOS tar adds ._ entries for extended attributes, and
# they would land inside the installed bundle as stray files.
COPYFILE_DISABLE=1 tar -czf "$out/$mac" -C "$(dirname "$app")" "$(basename "$app")"

sign() {
  rm -f "$1.sig"
  (cd "$here/desktop" && \
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" npx tauri signer sign -f "$key" -p "" "$1" >/dev/null)
  [ -s "$1.sig" ] || { echo "!! no signature written for $1" >&2; exit 1; }
}
sign "$out/$mac"
sign "$out/$win"

python3 - "$out" "$version" "$repo" "$mac" "$win" <<'PY'
import datetime, json, sys
out, version, repo, mac, win = sys.argv[1:]
sig = lambda f: open(f"{out}/{f}.sig").read().strip()
feed = {
    "version": version,
    "notes": f"https://github.com/furyteamtop/fury-antidetect-browser/releases/tag/v{version}",
    "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "platforms": {
        "darwin-aarch64": {"signature": sig(mac), "url": f"{repo}/{mac}"},
        "windows-x86_64": {"signature": sig(win), "url": f"{repo}/{win}"},
    },
}
json.dump(feed, open(f"{out}/latest.json", "w"), indent=2)
print(f"wrote {out}/latest.json")
PY
ls -la "$out/$mac" "$out/$mac.sig" "$out/$win.sig" "$out/latest.json"
