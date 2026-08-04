#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# Sign and notarise the core for macOS.
#
# This drives Chromium's own signing pipeline rather than calling codesign, and
# that is the entire point of the file. `codesign --deep --force` on a Chrome
# bundle produces something that installs, launches once, and then fails in ways
# that look like anything but a signing problem — Google's own documentation
# says --deep is wrong for this bundle, because the helpers need different
# entitlements from each other and from the outer app: the renderer helper wants
# JIT, the GPU helper wants unsigned executable memory, the plugin helper wants
# library validation off, and --deep gives every one of them whatever the outer
# app had.
#
# chrome/installer/mac/sign_chrome.py knows all of that. It signs inside-out —
# helpers, then the framework, then the app — attaches the right entitlement
# plist to each part, and can submit the result to Apple and staple the ticket.
#
# Requires an Apple Developer account ($99/year) for a Developer ID Application
# certificate. There is no useful substitute, and this was measured rather than
# assumed: --adhoc runs the pipeline to completion, codesign --verify --deep
# --strict passes, and the resulting browser DOES NOT START.
#
#   dlopen(.../Fury Framework): code signature not valid for use in process:
#   mapping process and mapped file (non-platform) have different Team IDs
#
# The pipeline signs the app with library validation on, which requires every
# library it loads to carry the same Team ID. A Developer ID has one and all
# the parts get it. An ad-hoc signature has none, so the app is forbidden from
# loading its own framework. Chrome ships this way and it is correct; it simply
# cannot be exercised without a certificate.
#
# For local development, do nothing: the linker already ad-hoc signs the build
# (`flags=0x20002(adhoc,linker-signed)`), and that signature works because it
# does not set library validation. --adhoc here is for checking that the
# pipeline itself runs and for reading the entitlements it attaches, and the
# script says so and then proves it by running the result.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
src="$here/core/src"

out_dir="${OUT_DIR:-$src/out/macos-arm64.noindex}"
identity=""
adhoc=0
notarize=0
keychain_profile="${FURY_NOTARY_PROFILE:-fury-notary}"

usage() {
  cat <<'EOF'
usage: sign-core.sh [--identity NAME | --adhoc] [--notarize] [--out DIR]

  --identity NAME     Developer ID Application certificate, e.g.
                      "Developer ID Application: Your Name (TEAMID)".
                      `security find-identity -v -p codesigning` lists them.
  --adhoc             Run the pipeline with no certificate. The output will
                      NOT run — see the comment at the top. Use it to check
                      the pipeline and to inspect entitlements.
  --notarize          Submit to Apple and staple the ticket. Needs --identity
                      and a stored notarytool profile — see below.
  --out DIR           Build output directory (default out/macos-arm64.noindex).

Store the notary credentials once, in the keychain, not in this script:

  xcrun notarytool store-credentials fury-notary \
      --apple-id you@example.com \
      --team-id TEAMID \
      --password <app-specific-password>

The app-specific password comes from appleid.apple.com, not your Apple ID
password. It only grants notarisation.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --identity) identity="${2:?--identity needs a value}"; shift 2 ;;
    --adhoc)    adhoc=1; identity="-"; shift ;;
    --notarize) notarize=1; shift ;;
    --out)      out_dir="${2:?--out needs a value}"; shift 2 ;;
    -h|--help)  usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [ -z "$identity" ]; then
  echo "!! pass --identity NAME or --adhoc" >&2
  echo >&2
  echo "Certificates on this machine:" >&2
  security find-identity -v -p codesigning >&2 || true
  exit 2
fi

if [ "$adhoc" = 1 ] && [ "$notarize" = 1 ]; then
  # Worth refusing rather than discovering after the upload: Apple notarises
  # what a Developer ID signed. An ad-hoc signature has no team behind it and
  # the submission is rejected, several minutes in.
  echo "!! --adhoc cannot be notarised: Apple notarises Developer ID signatures" >&2
  exit 2
fi

app="$out_dir/Fury.app"
[ -d "$app" ] || { echo "!! no $app — build the core first (core/build/build.sh)" >&2; exit 1; }

if [ "$adhoc" = 1 ]; then
  echo "note: an ad-hoc pipeline signature produces a browser that does not start." >&2
  echo "      Library validation requires a Team ID and ad-hoc has none. This is" >&2
  echo "      for checking the pipeline; for local use the build is already signed." >&2
  echo >&2
fi

# The pipeline uses asyncio.TaskGroup, which is Python 3.11. macOS ships 3.9
# (Xcode's), and depot_tools' vpython3 is 3.8 — so on a machine that has built
# Chromium and nothing else, neither obvious python works. Without this check
# the failure is `AttributeError: module 'asyncio' has no attribute 'TaskGroup'`
# eight frames into someone else's code, which reads like a broken checkout.
PY=""
for candidate in "${FURY_PYTHON:-}" python3.13 python3.12 python3.11 python3; do
  [ -n "$candidate" ] || continue
  command -v "$candidate" >/dev/null 2>&1 || continue
  if "$candidate" -c 'import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)' 2>/dev/null; then
    PY="$candidate"; break
  fi
done
if [ -z "$PY" ]; then
  echo "!! Chromium's signing pipeline needs Python 3.11 or newer (it uses asyncio.TaskGroup)." >&2
  echo "   This machine has $(python3 -V 2>&1), which is macOS's own." >&2
  echo "   Install one — brew install python@3.12 — or set FURY_PYTHON to a newer interpreter." >&2
  exit 1
fi

# The pipeline that gets run is the one in the build output, NOT the one in the
# source tree. They are not interchangeable: signing/build_props_config.py is
# generated by GN from a template and records what this build actually is —
# branded or not, updater or not, the product name. The copy in the source tree
# does not exist, so running chrome/installer/mac/sign_chrome.py directly fails
# with ModuleNotFoundError several frames deep in an import, which reads like a
# broken checkout and is not one.
#
# It is built by a target of its own, because `ninja chrome` does not produce
# it — the first attempt here signed nothing for that reason.
packaging="$out_dir/Fury Packaging"
if [ ! -f "$packaging/sign_chrome.py" ]; then
  echo "== building the packaging tools (they are not part of \`ninja chrome\`)"
  (cd "$src" && ./third_party/ninja/ninja -C "$(basename "$out_dir" | sed "s|^|out/|")" chrome/installer/mac)
fi
[ -f "$packaging/sign_chrome.py" ] || {
  echo "!! no $packaging/sign_chrome.py" >&2
  echo "   build it: cd core/src && ./third_party/ninja/ninja -C <outdir> chrome/installer/mac" >&2
  exit 1
}

staged="$out_dir/signed"
rm -rf "$staged"
mkdir -p "$staged"

echo "== signing $app"
echo "   identity: $identity"

args=(
  "$packaging/sign_chrome.py"
  --input "$out_dir"
  --output "$staged"
  --identity "$identity"
  # The .dmg the pipeline can build is Chrome's, with Chrome's background image
  # and Chrome's layout. Fury ships a tarball instead (package.sh), so the
  # packaging step is skipped and only the signed bundle is taken.
  --disable-packaging
)

if [ "$adhoc" = 1 ]; then
  # Skips the requirements that only mean something with a real certificate:
  # spctl assessment, provisioning profiles, pinned executable geometry.
  args+=(--development)
fi

if [ "$notarize" = 1 ]; then
  args+=(--notarize staple --notary-arg "--keychain-profile" --notary-arg "$keychain_profile")
fi

"$PY" "${args[@]}"

# The signed bundle lands under a channel directory — `signed/stable/Fury.app`
# — because the pipeline is built to emit several channels at once and "stable"
# is what the None channel is called on disk. Looking for it at the top level
# finds nothing after a signing run that succeeded completely.
signed="$staged/stable/Fury.app"
[ -d "$signed" ] || signed="$staged/Fury.app"
[ -d "$signed" ] || {
  echo "!! the pipeline reported success and produced no Fury.app. Under $staged:" >&2
  find "$staged" -maxdepth 2 >&2
  exit 1
}

echo
echo "== verifying"
# --deep is right for *verification* even though it is wrong for signing: here
# it means "check every nested bundle too", which is exactly what is wanted.
codesign --verify --deep --strict --verbose=2 "$signed"

if [ "$adhoc" = 0 ]; then
  # What Gatekeeper will actually say on a machine that has never seen this
  # binary. `codesign --verify` passing and this failing is the normal shape of
  # a signed-but-not-notarised build, and it is the failure users report.
  echo
  echo "== Gatekeeper assessment"
  spctl --assess --type execute --verbose=2 "$signed" || {
    echo "   (not accepted — sign with a Developer ID and --notarize for that)" >&2
  }
fi

if [ "$notarize" = 1 ]; then
  echo
  echo "== stapled ticket"
  # The ticket has to be stapled for the bundle to pass on a machine that is
  # offline at first launch, which is the case this catches.
  xcrun stapler validate "$signed"
fi

echo
# The check that separates a signed bundle from a working one, and the reason
# this script has it: everything above passed on the ad-hoc run and the browser
# aborted on launch. codesign --verify answers "is this signature well formed",
# which is a different question from "will the loader accept it".
echo "== does it start"
if version=$("$signed/Contents/MacOS/Fury" --version 2>&1); then
  echo "   $version"
else
  echo "!! the signed browser does not start:" >&2
  echo "$version" | head -3 >&2
  if printf '%s' "$version" | grep -q "different Team IDs"; then
    echo >&2
    echo "   This is the ad-hoc case: library validation needs a Team ID." >&2
    echo "   Sign with a Developer ID Application certificate." >&2
  fi
  exit 1
fi

echo
echo "signed bundle: $signed"
echo "package it with: tools/release/package.sh --core \"$signed\""
