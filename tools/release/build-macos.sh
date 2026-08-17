#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# Build the macOS shell: the application, then the disk image, as two runs.
#
#     tools/release/build-macos.sh          # both
#     tools/release/build-macos.sh --app    # just the .app, for installing here
#
# WHY TWO RUNS AND NOT `npm run app:build`.
#
# Asking tauri for both bundles at once fails at the second one, every time, with
# the whole of its explanation being:
#
#     Running bundle_dmg.sh
#     failed to bundle project: error running bundle_dmg.sh
#
# Asking for them separately works, every time. Measured on 17.08.2026: three
# failures from the combined run, three successes from the split one, on the
# same tree with the same tools.
#
# WHAT I DID NOT ESTABLISH is why, and this comment says so rather than
# inventing a cause. Two things were ruled out by looking rather than guessing:
# a leftover scratch volume from an interrupted build, which really does break
# the next one and is swept by install-macos.sh, and a stale rw.*.dmg beside it.
# Neither was present for the last failure. The remaining suspicion is that
# tauri closes the pipe it gave the script while the script is still writing to
# it -- bundle_dmg.sh runs under `set -o pipefail`, so a write to a closed
# stdout would end it non-zero with nothing to show for it -- but that is a
# hypothesis about somebody else's code and it is written here as one.
#
# The split is cheap, it is reliable, and a build system that works for a reason
# you have not fully proven is better than one that fails for a reason nobody
# has written down.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$here/desktop"

# Homebrew's node is not on PATH in a non-interactive shell.
export PATH="/opt/homebrew/bin:$PATH"

# Same sweep install-macos.sh does, and needed BEFORE the build rather than
# after: a volume left mounted by an interrupted run breaks the next packaging
# step, and that is the one failure here with a known cause.
for v in /Volumes/dmg.*; do
  [ -d "$v" ] || continue
  echo "==> detaching a leftover build volume: $v"
  hdiutil detach "$v" -quiet 2>/dev/null || hdiutil detach "$v" -force >/dev/null 2>&1 || true
done
rm -f "$here/target/release/bundle/macos"/rw.*.dmg

echo "==> building the application"
npx tauri build --bundles app

if [ "${1:-}" = "--app" ]; then
  echo "==> stopping there, as asked"
  exit 0
fi

# This rebuilds the .app as well, which is a few seconds and not worth avoiding:
# tauri owns that step and asking it to skip one is how the combined run got
# into trouble in the first place.
echo "==> building the disk image"
npx tauri build --bundles dmg

ls -la "$here/target/release/bundle/dmg"/*.dmg
