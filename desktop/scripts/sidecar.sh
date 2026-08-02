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
cargo build --release -p fury-agent
mkdir -p desktop/src-tauri/binaries
cp target/release/fury-agent "desktop/src-tauri/binaries/fury-agent-${TRIPLE}"
echo "==> sidecar: fury-agent-${TRIPLE}"
