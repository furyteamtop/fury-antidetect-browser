#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# Sign and notarise the desktop shell for macOS.
#
# The core has its own script (sign-core.sh) and it is a different problem:
# there the work is driving Chromium's signing pipeline, because a Chrome bundle
# needs different entitlements on each helper. Here the bundler already knows how
# to sign — measured, not assumed: the Tauri CLI binary contains the literal
# `/usr/bin/codesign`, `--force`, `--options`, `runtime` and `--entitlements`,
# reads APPLE_SIGNING_IDENTITY, and recognises `Developer ID Application:`
# identities. `hardenedRuntime` defaults to true in the config schema, so the
# hardened runtime is on without anyone setting it.
#
# So this script exists for the part the bundler does NOT do reliably: the
# sidecar, and proving afterwards that the result is actually shippable.
#
# WHAT WAS WRONG, measured 15.08.2026 on the bundle in target/release:
#
#   Contents/MacOS/fury-desktop   flags=0x20002(adhoc,linker-signed)  TeamIdentifier=not set
#   Contents/MacOS/fury-agent     flags=0x20002(adhoc,linker-signed)  TeamIdentifier=not set
#
# Both binaries carried the signature the linker leaves behind, which is not a
# Developer ID and has no team. That bundle cannot be notarised and would trip
# Gatekeeper on every machine that downloaded it.
#
# WHY THE IDENTITY IS NOT IN tauri.conf.json. The bundler would read it there,
# and it is the obvious place, and it is wrong for this repository: the string
# is `Developer ID Application: <legal name> (TEAMID)`, so committing it prints
# a personal name and a team into a public AGPL tree forever. It travels in the
# environment instead, and the same variable reaches the sidecar script, so the
# app and the binary inside it cannot end up with different teams.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
app="$here/target/release/bundle/macos/Fury.app"
identity="${APPLE_SIGNING_IDENTITY:-}"
notarize=0
build=1
keychain_profile="${FURY_NOTARY_PROFILE:-fury-notary}"

usage() {
  cat <<'EOF'
usage: sign-shell.sh [--identity NAME] [--notarize] [--skip-build]

  --identity NAME  Developer ID Application certificate, e.g.
                   "Developer ID Application: Your Name (TEAMID)".
                   Defaults to $APPLE_SIGNING_IDENTITY.
                   `security find-identity -v -p codesigning` lists them.
  --notarize       Submit to Apple and staple the ticket. Needs a stored
                   notarytool profile (see sign-core.sh --help for how).
  --skip-build     Verify and notarise the bundle that is already there.
                   Use it to re-check, never to produce a release: the
                   signature is applied BY the build, so a skipped build
                   verifies whatever the last one happened to leave.

The identity is passed in the environment rather than written into
tauri.conf.json on purpose — see the comment at the top of this file.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --identity)   identity="${2:?--identity needs a value}"; shift 2 ;;
    --notarize)   notarize=1; shift ;;
    --skip-build) build=0; shift ;;
    -h|--help)    usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [ -z "$identity" ]; then
  echo "!! pass --identity NAME or set APPLE_SIGNING_IDENTITY" >&2
  echo >&2
  echo "Certificates on this machine:" >&2
  security find-identity -v -p codesigning >&2 || true
  exit 2
fi

# Fail before a six-minute build rather than after it. An identity that is not
# in the keychain produces a bundler error deep in a Rust backtrace, and the
# common cause is mundane: the certificate is issued but was never downloaded
# and double-clicked, so it exists at Apple and not here.
if ! security find-identity -v -p codesigning | grep -qF "$identity"; then
  echo "!! no such identity in the keychain: $identity" >&2
  echo >&2
  security find-identity -v -p codesigning >&2 || true
  exit 2
fi

# The team the certificate belongs to, taken from the identity string itself:
# `Developer ID Application: Name (TEAMID)`. Used below to prove the sidecar and
# the app ended up on the same team, which is the failure this script is for.
team="$(printf '%s' "$identity" | sed -n 's/.*(\([A-Z0-9]*\))$/\1/p')"
if [ -z "$team" ]; then
  echo "!! could not read a Team ID out of: $identity" >&2
  echo "   expected the form: Developer ID Application: Name (TEAMID)" >&2
  exit 2
fi

if [ "$build" = 1 ]; then
  echo "== building the shell with APPLE_SIGNING_IDENTITY set"
  echo "   identity: $identity"
  # Exported, not passed as an argument, because two consumers read it: the
  # bundler for the outer app, and desktop/scripts/sidecar.mjs — which runs as
  # beforeBuildCommand — for the agent binary. Signing must go inside-out, and
  # this is what makes that happen in one build.
  export APPLE_SIGNING_IDENTITY="$identity"
  (cd "$here/desktop" && npm run app:build)
fi

[ -d "$app" ] || { echo "!! no $app — build the shell first" >&2; exit 1; }

echo
echo "== verifying $app"
# --deep is right for verification even though it is wrong for signing: here it
# means "check every nested binary too", which is the whole question.
codesign --verify --deep --strict --verbose=2 "$app"

echo
echo "== every executable in the bundle"
# The check this file exists for. `codesign --verify --deep` on the app answers
# "is the seal intact", which an adhoc-signed nested binary can still satisfy in
# a locally built bundle. It does not answer "is every part on OUR team", and
# that is the question Apple asks at notarisation and the loader asks at launch.
fail=0
while IFS= read -r bin; do
  got="$(codesign -dv --verbose=2 "$bin" 2>&1 | sed -n 's/^TeamIdentifier=//p')"
  name="${bin#"$app/"}"
  if [ "$got" = "$team" ]; then
    printf '   ok       %-40s %s\n' "$name" "$got"
  else
    printf '   MISMATCH %-40s %s\n' "$name" "${got:-none}"
    fail=1
  fi
done < <(find "$app/Contents/MacOS" -type f -perm -u+x)

if [ "$fail" = 1 ]; then
  echo >&2
  echo "!! a binary in the bundle is not signed by team $team." >&2
  echo "   Notarisation rejects this at upload, minutes in, naming a path and" >&2
  echo "   not a cause. If it is fury-agent, the sidecar was built without" >&2
  echo "   APPLE_SIGNING_IDENTITY — rebuild without --skip-build." >&2
  exit 1
fi

echo
echo "== hardened runtime"
# Mandatory for notarisation, and silently absent is exactly how it goes wrong:
# a bundle signs and verifies happily without it and is refused on upload.
if codesign -d --verbose=2 "$app" 2>&1 | grep -q "flags=.*runtime"; then
  echo "   enabled"
else
  echo "!! hardened runtime is NOT enabled on $app" >&2
  echo "   bundle.macOS.hardenedRuntime defaults to true; something turned it off." >&2
  exit 1
fi

echo
echo "== Gatekeeper assessment"
# What a machine that has never seen this bundle will say. `codesign --verify`
# passing and this failing is the normal shape of signed-but-not-notarised, and
# it is the failure users report.
spctl --assess --type execute --verbose=2 "$app" || {
  echo "   (not accepted — that is expected until --notarize has run)" >&2
}

if [ "$notarize" = 1 ]; then
  echo
  echo "== notarising"
  # notarytool does not take a bare .app. ditto is what Apple documents for
  # this; `zip -r` mangles symlinks and extended attributes inside a bundle,
  # and the resulting submission fails in a way that reads as a signing fault.
  zip="$here/target/release/bundle/macos/Fury.zip"
  rm -f "$zip"
  ditto -c -k --keepParent "$app" "$zip"

  xcrun notarytool submit "$zip" --keychain-profile "$keychain_profile" --wait

  # Staple the .app, not the zip: the zip is a transport, and what a user keeps
  # is the bundle. Without a stapled ticket a first launch with no network
  # fails, because Gatekeeper cannot reach Apple to ask.
  xcrun stapler staple "$app"
  xcrun stapler validate "$app"
  rm -f "$zip"

  echo
  echo "== Gatekeeper, after stapling"
  spctl --assess --type execute --verbose=2 "$app"
fi

echo
echo "signed shell: $app"
echo "package it with: tools/release/package.sh --shell \"$app\""
