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
allow_unsigned=0

usage() {
  cat <<'EOF'
usage: package.sh [--core PATH] [--shell PATH] [--version V] [--out DIR]

  --core PATH     A signed Fury.app for the browser core. Produce one with
                  tools/release/sign-core.sh.
  --shell PATH    The desktop application, from `npm run app:build`. Prefer the
                  installer that build produced — target/release/bundle/dmg/*.dmg
                  on macOS, bundle/nsis/*-setup.exe on Windows — which is copied
                  through as-is. A Fury.app directory is also accepted and gets
                  tarred, which is what you have if you built `app` only.
  --version V     Release version. Default: the agent crate's version.
  --out DIR       Where to write (default dist/).
  --allow-unsigned
                  Package a macOS shell that is only ad-hoc signed. Off by
                  default, because a downloaded ad-hoc bundle is refused by
                  Gatekeeper with "is damaged" and the person who downloaded it
                  has no way to know that a signature is what is missing.

Either may be omitted; whatever is given gets packaged.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --core)    core_app="${2:?--core needs a path}"; shift 2 ;;
    --shell)   shell_app="${2:?--shell needs a path}"; shift 2 ;;
    --version) version="${2:?--version needs a value}"; shift 2 ;;
    --out)     out="${2:?--out needs a path}"; shift 2 ;;
    --allow-unsigned) allow_unsigned=1; shift ;;
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
# The shape is checked rather than the character set. `[!0-9.]` stood here and
# rejected every pre-release version there is — 0.1.0-pre1 among them — which
# made this script unable to express the one kind of release that comes first.
#
# The guard it replaces still holds: what it was written to catch is
# `version.workspace = true` reaching a filename, and that has letters, spaces
# and an `=`, so an anchored MAJOR.MINOR.PATCH[-suffix] refuses it just as
# firmly. The suffix is deliberately narrow — alphanumerics and dots — so a
# stray shell expansion cannot arrive as a "version" and end up in a name.
if ! printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'; then
  echo "!! could not read a version (got \"$version\") — pass --version" >&2
  echo "   expected 1.2.3, optionally with a suffix: 1.2.3-pre1" >&2
  exit 1
fi

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

# The shell now arrives in whatever form its platform actually installs from,
# and the argument may be a FILE rather than a directory.
#
# `tauri build` emits .dmg on macOS and the NSIS .exe on Windows (both are in
# bundle.targets), and either is already the finished artifact: signed, and in
# the DMG's case stapleable, which a tarball is not — a .tar.xz carries no
# notarisation ticket of its own, only the .app inside it does. Re-wrapping a
# .dmg in a tarball would also hand a Mac user two unpacking steps to reach a
# window that exists to have exactly one.
#
# The directory branch is kept rather than replaced: a bare .app is still what
# you have if you built with only the `app` target, and refusing it would turn a
# working local packaging run into an error about a missing file.
# Refuse to publish a shell that macOS will not open, and refuse to publish one
# that lies about its version.
#
# Both of these shipped in v0.1.0-pre1..pre3 and both were found by a person
# rather than by this script, which is why the checks are here now.
#
# 1. THE SIGNATURE. `tauri build` without a signing identity emits a bundle whose
#    executable carries only the linker's ad-hoc signature while the bundle
#    itself is not signed at all — `Sealed Resources=none`, `Info.plist=not
#    bound`. Nothing complains locally, because a file you built yourself has no
#    quarantine attribute. A file somebody DOWNLOADS does, and quarantine plus an
#    invalid bundle signature produces the flat refusal:
#
#      "Приложение «Fury» повреждено, и его не удаётся открыть."
#
#    The user is told to move it to the Trash. Nothing in that message suggests a
#    signature, so the natural reading is that the software is broken — and the
#    natural next step, reinstalling, reproduces it exactly.
#
# 2. THE VERSION. The release version reached the FILE NAMES and never the
#    bundle: tauri.conf.json carried its own hardcoded "0.0.1", so
#    fury-0.1.0-pre3-macos-arm64.dmg installed an application that reported
#    0.0.1 to itself. The in-app update check compares against that number, so
#    it answered about a version nobody was running. tauri.conf.json no longer
#    holds a version — it inherits from [workspace.package] — and this check is
#    what keeps the two from drifting apart again.
verify_shell_app() {
  local app="$1"
  [ "$platform" = "macos" ] || return 0

  local plist="$app/Contents/Info.plist"
  local said
  said=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$plist" 2>/dev/null || echo "")
  # A pre-release suffix cannot go in CFBundleShortVersionString: Apple wants
  # digits and dots there. Compare against the numeric part and let the suffix
  # live in the tag and the file name.
  local numeric="${version%%-*}"
  if [ "$said" != "$numeric" ]; then
    echo "!! the bundle says version \"$said\", the release is \"$version\"" >&2
    echo "!! whoever installs this gets an app that reports the wrong version to" >&2
    echo "!! itself, and the update check then answers about a version nobody runs." >&2
    exit 1
  fi
  echo "   bundle version $said"

  if ! codesign -v --deep --strict "$app" 2>/dev/null; then
    echo "!! the bundle signature is not valid:" >&2
    codesign -v --deep --strict "$app" 2>&1 | sed 's/^/!!   /' >&2
    echo "!! macOS will refuse to open this after download, with \"is damaged\"." >&2
    echo "!! Sign it: tools/release/sign-shell.sh --identity 'Developer ID Application: ...'" >&2
    exit 1
  fi

  # Valid but ad-hoc is still not shippable. An ad-hoc signature has no team
  # behind it, so Gatekeeper rejects it on a downloaded file exactly like an
  # invalid one — the only difference is that this one passes the check above.
  if codesign -dv "$app" 2>&1 | grep -q 'Signature=adhoc'; then
    if [ "${allow_unsigned:-0}" = "1" ]; then
      echo "   WARNING: ad-hoc signature, --allow-unsigned given"
      echo "   Anyone who downloads this must run:"
      echo "     xattr -dr com.apple.quarantine /Applications/Fury.app"
    else
      echo "!! this bundle is ad-hoc signed, which is not distributable: a" >&2
      echo "!! downloaded copy is quarantined and Gatekeeper refuses it." >&2
      echo "!! Sign with a Developer ID and notarise (tools/release/sign-shell.sh)," >&2
      echo "!! or pass --allow-unsigned if you accept that every downloader must" >&2
      echo "!! strip the quarantine attribute by hand." >&2
      exit 1
    fi
  fi
}

package_shell() {
  local app="$1"
  local name

  if [ -f "$app" ]; then
    # Already an installer. Keep the extension it came with — the whole point is
    # that macOS sees .dmg and Windows sees .exe.
    #
    # Look INSIDE the .dmg rather than at the bundle it was made from. They are
    # supposed to be the same application and the checks are about what a person
    # actually downloads, so the copy that ships is the copy to inspect.
    if [ "${app##*.}" = "dmg" ]; then
      echo "== checking the application inside the disk image"
      local mnt
      mnt=$(mktemp -d)
      hdiutil attach -nobrowse -readonly -mountpoint "$mnt" "$app" >/dev/null
      # Detach even if a check exits — an image left mounted registers its copy
      # of Fury.app with LaunchServices, and then the machine has two.
      trap 'hdiutil detach "$mnt" -quiet 2>/dev/null || true; rmdir "$mnt" 2>/dev/null || true' RETURN
      local inner="$mnt/Fury.app"
      [ -d "$inner" ] || { echo "!! no Fury.app inside $app" >&2; exit 1; }
      verify_shell_app "$inner"
    fi

    name="fury-$version-$platform-$arch.${app##*.}"
    echo "== copying $name"
    cp "$app" "$out/$name"
  elif [ -d "$app" ]; then
    verify_shell_app "$app"
    name="fury-$version-$platform-$arch.tar.xz"
    echo "== packing $name"
    tar -cJf "$out/$name" -C "$(dirname "$app")" "$(basename "$app")"
  else
    echo "!! no such bundle: $app" >&2
    echo "   expected a .dmg or .exe from \`tauri build\`, or a Fury.app directory" >&2
    exit 1
  fi

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
#
# EVERYTHING, not ./*.tar.xz, which is what this said until the shell started
# shipping as a .dmg. The glob then matched nothing, shasum wrote an empty
# SHA256SUMS, and the failure was cosmetic in the output and severe in the
# artifact: docs/15 tells a downloader to run `shasum -a 256 -c SHA256SUMS` and
# expect OK lines, and an empty file gives them silence — a verification step
# that verifies nothing while looking like it ran. Caught by running package.sh
# against a real .dmg rather than by reading the change that caused it.
#
# find rather than a glob so the failure mode is an empty release directory
# refusing loudly, not a shell passing an unmatched pattern through as a literal.
shopt -s nullglob
artifacts=("$out"/*.tar.xz "$out"/*.dmg "$out"/*.exe "$out"/*.msi)
shopt -u nullglob
if [ ${#artifacts[@]} -eq 0 ]; then
  echo "!! nothing was packaged into $out — no checksums to write" >&2
  exit 1
fi
(cd "$out" && shasum -a 256 $(printf './%s\n' "${artifacts[@]##*/}") > SHA256SUMS)
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

# CORE_BINARY is derived here. It used to be referenced on the line below and
# assigned nowhere at all, so `--core` never reached the report: every release
# document said
#
#     | core        | (unknown)                     |
#     | core sha256 | (not given — pass --core)     |
#
# including the ones built WITH --core, advising the operator to pass a flag
# they had just passed. `${VAR:+...}` is exempt from `set -u`, so nothing warned.
#
# The consequence is not cosmetic. REPORT.md says of itself that "the hashes
# above are what make it checkable rather than believable", and the core's hash
# is the one that matters — a reader can rebuild the patch series and re-take a
# capture, but without the core's digest they cannot tell whether the document
# describes the binary they downloaded. A verifiability claim with the evidence
# missing is worse than no claim.
CORE_BINARY="${CORE_BINARY:-}"
if [ -z "$CORE_BINARY" ] && [ -n "$core_app" ]; then
  CORE_BINARY="$core_app/Contents/MacOS/Fury"
  [ -x "$CORE_BINARY" ] || CORE_BINARY=""
fi

if [ -f "$CAPTURE" ]; then
  echo "==> measurement report"
  cargo run -q -p fury-detect -- report "$CAPTURE" \
    ${CORE_BINARY:+--core "$CORE_BINARY"} \
    --out "$out/REPORT.md" || echo "!! the gate failed — see $out/REPORT.md"
else
  echo "==> no capture at $CAPTURE — the release will carry no measurement report"
fi
