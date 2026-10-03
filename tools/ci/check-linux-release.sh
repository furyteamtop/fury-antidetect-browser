#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# The Linux release contract, checked statically on a hosted runner.
#
# The asset name is agreed between four places that never compile together:
# the agent's downloader, the Linux packager, the release workflow and the
# tauri bundle targets. A rename in one of them is not a compile error — it is
# a user pressing the download button and getting "no core asset in the latest
# release", or a release job silently producing a file nobody looks for. This
# pins the agreement, the same way check-repo-url.py pins the URLs.
#
# What it cannot check, honestly: that a Linux core or an AppImage actually
# builds and runs. That is linux-core.yml and linux-release.yml, on the
# runners that can.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

expected=linux-x64

# The consumer: what the downloader asks the releases API for.
grep -q '"linux-x64"' "$here/agent/src/core_download.rs"

# The producers: the packager's three artifact names and the build command.
grep -q "fury-core-\$version-$expected.tar.xz" "$here/tools/release/package-linux.sh"
grep -q "fury-\$version-$expected.AppImage" "$here/tools/release/package-linux.sh"
grep -q "fury-\$version-$expected.deb" "$here/tools/release/package-linux.sh"
grep -q 'npx tauri build --bundles appimage,deb' "$here/tools/release/package-linux.sh"
grep -q 'FURY_CORE_DIR is required' "$here/tools/release/package-linux.sh"

# The workflow assembles the same three names and takes the core over HTTPS.
grep -q 'core_url:' "$here/.github/workflows/linux-release.yml"
grep -q "fury-core-.*-$expected.tar.xz" "$here/.github/workflows/linux-release.yml"

# The core target exists in the build: args file, build.sh case, packer.
test -f "$here/core/args/linux-x64.gn"
grep -q 'linux-x64\*)' "$here/core/build/build.sh"
test -f "$here/tools/release/pack-core-linux.sh"
# And the consumer's launcher-side leaf names agree with the flat archive the
# packer produces: chrome at the top, found by direct join.
grep -q '"fury-core", "chrome"' "$here/agent/src/main.rs"

python3 - "$here/desktop/src-tauri/tauri.conf.json" <<'PY'
import json, sys
config = json.load(open(sys.argv[1]))
targets = config["bundle"]["targets"]
assert "appimage" in targets and "deb" in targets, targets
PY

bash -n "$here/tools/release/package-linux.sh" "$here/tools/release/pack-core-linux.sh" "$here/core/build/build.sh"
echo 'linux release contract: ok'
