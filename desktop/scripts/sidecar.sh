#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

# Build the agent and place it where the bundler expects a sidecar.
#
# This exists because `tauri build` rewrites the .app from scratch: a binary
# copied in by hand after a build survives exactly until the next one, which is
# how the shipped app twice lost the agent it depends on and then reported "the
# local agent is not running" on a machine where it had worked minutes earlier.
# The bundler has to own the copy.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

TRIPLE="$(rustc -vV | awk '/^host:/{print $2}')"

# Tauri looks for `fury-agent-<triple><exe suffix>` and the suffix is not
# optional on Windows: a sidecar named without `.exe` is a sidecar the bundler
# reports as missing, in a message that names the triple and not the extension.
#
# Run this from Git Bash on Windows. A second script in PowerShell was the
# alternative and was rejected: two scripts that must stay in step is the
# arrangement that put the agent address in two files and let them drift.
case "$TRIPLE" in
  *windows*) EXE=".exe" ;;
  *)         EXE="" ;;
esac

cargo build --release -p fury-agent
mkdir -p desktop/src-tauri/binaries
cp "target/release/fury-agent${EXE}" "desktop/src-tauri/binaries/fury-agent-${TRIPLE}${EXE}"
echo "==> sidecar: fury-agent-${TRIPLE}${EXE}"

# Sign the sidecar HERE, before `tauri build` runs, and not afterwards.
#
# Code signatures nest inside-out: the outer bundle's signature seals its
# contents, so signing a binary that is already inside a signed .app invalidates
# the .app. Signing the source file instead means the signature travels with the
# copy the bundler makes, and the bundler then seals an already-signed sidecar.
#
# This matters because the bundler signs the app, and Apple requires EVERY
# executable in the bundle to be signed — including nested ones. An unsigned
# sidecar is the second most common notarisation rejection after a stray
# get-task-allow, and it fails at upload, several minutes in, with a message
# that names a path and not a cause.
#
# Both flags are mandatory for notarisation, not preferences: --options runtime
# enables the hardened runtime, --timestamp attaches the secure timestamp that
# lets the signature keep validating after the certificate expires.
#
# APPLE_SIGNING_IDENTITY is the same variable the Tauri bundler reads, and that
# is deliberate: the sidecar and the app it lives in must carry the SAME Team
# ID, or library validation refuses the bundle. One variable cannot drift from
# itself; two would, and this file already carries a comment about what drift
# costs.
if [ "${TRIPLE#*darwin}" != "$TRIPLE" ]; then
  sidecar="desktop/src-tauri/binaries/fury-agent-${TRIPLE}${EXE}"
  if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    echo "==> signing sidecar with ${APPLE_SIGNING_IDENTITY}"
    codesign --force --sign "$APPLE_SIGNING_IDENTITY" \
             --options runtime --timestamp "$sidecar"
    codesign --verify --strict --verbose=2 "$sidecar"
  else
    # Not an error: a developer building locally has no certificate and must
    # still get a working app. Said out loud because a release built without it
    # fails notarisation LATER, and a silent skip here is what makes that
    # failure look like it came from somewhere else.
    echo "==> sidecar NOT signed: APPLE_SIGNING_IDENTITY is unset."
    echo "    Fine for local builds. A release build needs it — see docs/17."
  fi
fi
