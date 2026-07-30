#!/usr/bin/env bash
# The monthly treadmill: move the patch series onto a new Chromium release.
#
# Usage: core/build/rebase.sh 151.0.7842.60
#
# Budget half a day to two days depending on how much upstream moved. Patches
# marked '!' in the series file touch fast-moving upstream code and are the
# usual suspects.
set -euo pipefail

CORE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NEW_VERSION="${1:?usage: rebase.sh <chromium-tag>}"
OLD_VERSION="$(cat "$CORE_DIR/CHROMIUM_VERSION" 2>/dev/null || echo unknown)"

echo "==> Rebase $OLD_VERSION -> $NEW_VERSION"
echo

# Which patches are likely to break, per the series annotations.
echo "Patches flagged as touching hot upstream files:"
grep '!$' "$CORE_DIR/patches/series" | sed 's/!$//' | sed 's/^/  /'
echo

read -rp "Continue? [y/N] " ans
[ "$ans" = "y" ] || exit 0

"$CORE_DIR/build/fetch.sh" "$NEW_VERSION"

echo
echo "==> Dry run to see the damage before touching anything"
if "$CORE_DIR/build/apply.sh" --check; then
  echo "==> Everything still applies. Applying for real."
  "$CORE_DIR/build/apply.sh"
else
  cat <<EOF

==> Some patches need work. For each one:

      cd $CORE_DIR/src
      git apply --3way patches/<name>.patch
      # resolve conflict markers
      $CORE_DIR/build/refresh.sh <name>

    Then re-run: $CORE_DIR/build/apply.sh --check

EOF
  exit 1
fi

cat <<EOF

==> Patches applied. Do NOT ship yet. Remaining steps:

    1. Build:   core/build/build.sh macos-arm64
    2. Re-capture the TLS/H2 reference from a REAL Chrome $NEW_VERSION.
       A patch can apply cleanly and still be dead code after upstream renames.
    3. Run the detect suite. Release gate is docs/07-detection-baseline.md.
    4. Only then sign, notarise, publish.

EOF
