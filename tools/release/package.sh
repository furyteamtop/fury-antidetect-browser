#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# Build the files a release consists of, and the checksums for them.
#
# A release is two downloads, not one, and that is a decision rather than an
# accident. The core is 134 MB compressed against the shell's 12 MB, and the two
# change on completely different schedules: the shell moves weekly, the core
# moves when Chromium does. Shipping them together would mean a 134 MB download
# for a button that moved.
#
# It also keeps the shell's code signature intact. A core written into a signed
# application bundle invalidates that bundle's seal, after which macOS refuses
# to open it and says the application is damaged — which sends people looking
# for a corrupt download instead of at us.
#
# GitHub Releases holds both. Measured rather than assumed: the limit is 2 GB
# per asset, the core is 134 MB as .tar.xz, so no CDN is needed and none is
# used. Anything that would require one is a decision to revisit here.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
out="${OUT:-$here/dist}"
core_app=""
shell_app=""
version=""

usage() {
  cat <<'EOF'
usage: package.sh [--core PATH] [--shell PATH] [--version V] [--out DIR]

  --core PATH     A signed Fury.app for the browser core. Produce one with
                  tools/release/sign-core.sh.
  --shell PATH    The desktop application bundle, from `npm run app:build`.
  --version V     Release version. Default: the agent crate's version.
  --out DIR       Where to write (default dist/).

Either may be omitted; whatever is given gets packaged.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --core)    core_app="${2:?--core needs a path}"; shift 2 ;;
    --shell)   shell_app="${2:?--shell needs a path}"; shift 2 ;;
    --version) version="${2:?--version needs a value}"; shift 2 ;;
    --out)     out="${2:?--out needs a path}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [ -z "$core_app" ] && [ -z "$shell_app" ]; then
  echo "!! nothing to package: pass --core, --shell, or both" >&2
  usage >&2
  exit 2
fi

if [ -z "$version" ]; then
  # The crates inherit their version from [workspace.package], so reading
  # agent/Cargo.toml gets `version.workspace = true` — which then went into a
  # filename, which is how this was found.
  version=$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f&&/^version/{gsub(/[^0-9.]/,"");print;exit}' \
            "$here/Cargo.toml")
fi
case "$version" in
  ''|*[!0-9.]*) echo "!! could not read a version (got \"$version\") — pass --version" >&2; exit 1 ;;
esac

arch=$(uname -m)
case "$(uname -s)" in
  Darwin) platform="macos" ;;
  Linux)  platform="linux" ;;
  *) echo "!! packaging is macOS and Linux only for now" >&2; exit 1 ;;
esac

mkdir -p "$out"

package_core() {
  local app="$1"
  [ -d "$app" ] || { echo "!! no such bundle: $app" >&2; exit 1; }

  # Refuse to package a core that does not run. The whole reason this check is
  # here: a bundle can pass `codesign --verify --deep --strict` and still abort
  # on launch, because verifying a signature and loading one are different
  # questions. Shipping that produces a first-run crash for every downloader.
  echo "== checking the core starts"
  local said
  if ! said=$("$app/Contents/MacOS/Fury" --version 2>&1); then
    echo "!! it does not:" >&2
    echo "$said" | head -3 >&2
    exit 1
  fi
  echo "   $said"

  local name="fury-core-$version-$platform-$arch.tar.xz"
  echo "== packing $name"
  # -J is xz. Measured against the alternatives on this bundle: 544 MB becomes
  # 134 MB, which is worth the ninety seconds; gzip gives about 200 MB in
  # fifteen. A download happens far more often than a compression does.
  #
  # -C so the archive contains Fury.app at the top and not somebody's absolute
  # path. `install-core` copes with either, but a person running tar by hand
  # gets what they expected.
  tar -cJf "$out/$name" -C "$(dirname "$app")" "$(basename "$app")"
  size_of "$out/$name"
}

package_shell() {
  local app="$1"
  [ -d "$app" ] || { echo "!! no such bundle: $app" >&2; exit 1; }

  local name="fury-$version-$platform-$arch.tar.xz"
  echo "== packing $name"
  tar -cJf "$out/$name" -C "$(dirname "$app")" "$(basename "$app")"
  size_of "$out/$name"
}

# du reports allocated blocks, which is not the number a download is measured
# in and disagrees with it by enough to be confusing.
size_of() {
  awk -v b="$(wc -c < "$1")" 'BEGIN{printf "   %.0f MB (%.0f MiB)\n", b/1000000, b/1048576}'
}

# `if` rather than `[ -n "$x" ] && f`, which was worth checking rather than
# assuming: under `set -e` bash does NOT exit on the guard failing here, because
# the test is the left operand of && and other commands follow. It would matter
# only if such a line were the last in the file, where the script would then
# exit 1 having done everything correctly. The `if` is simply clearer.
if [ -n "$core_app" ]; then package_core "$core_app"; fi
if [ -n "$shell_app" ]; then package_shell "$shell_app"; fi

echo
echo "== checksums"
# One file listing everything, in the format `shasum -c` reads, so verifying is
# a command a person can run rather than a hex string to compare by eye.
(cd "$out" && shasum -a 256 ./*.tar.xz > SHA256SUMS)
cat "$out/SHA256SUMS"

echo
echo "Verify a download with:"
echo "  shasum -a 256 -c SHA256SUMS"
echo
echo "in $out"

# ---------------------------------------------------------------------------
# The measurement report
# ---------------------------------------------------------------------------
# Beside the checksums, because a release that says "it spoofs" and a release
# that says WHAT WAS MEASURED, on WHICH BYTES, are different products.
#
# Competitors' evidence is screenshots and a table in a README, and neither can
# be re-taken by a reader: their binaries cannot be rebuilt or inspected. This
# names the core's sha256, the capture's, the patch series' and the commit, so
# a reader with the same release runs one command and compares documents.
#
# Skipped rather than fatal when there is no capture to report on. A release
# built on a machine that has not run the probe is a release with no
# measurement, and saying so beats inventing one.
CAPTURE="${FURY_CAPTURE:-tools/detect-suite/baselines/ctx-fury-redacted.json}"
if [ -f "$CAPTURE" ]; then
  echo "==> measurement report"
  cargo run -q -p fury-detect -- report "$CAPTURE" \
    ${CORE_BINARY:+--core "$CORE_BINARY"} \
    --out "$out/REPORT.md" || echo "!! the gate failed — see $out/REPORT.md"
else
  echo "==> no capture at $CAPTURE — the release will carry no measurement report"
fi
