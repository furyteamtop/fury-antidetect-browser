#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# A probe dump somebody sent you becomes a contributed persona, here, with no
# GitHub account on their side.
#
#     tools/persona-from-file.sh <capture.json> "<what machine>" [who] [weight]
#
#     tools/persona-from-file.sh ~/Downloads/fingerprint-Win32-153.json \
#         "Lenovo Legion 5, Windows 11, RTX 4060, built-in 1920x1080" "@vasya (TG)" 0.02
#
# The same steps .github/workflows/persona-issue.yml runs for an issue: convert
# with `fury-detect persona`, name it after the machine, write who sent it and
# what they said it was, refuse a reused id, run personas-check. The file lands
# in shared/personas/contributed/ ready to commit. Not everybody who owns a
# machine owns a GitHub account, and the catalogue should not depend on that.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$here"

capture="${1:?usage: persona-from-file.sh <capture.json> \"<what machine>\" [who] [weight]}"
machine="${2:?say what machine this is, e.g. \"MacBook Air M2 2022, built-in display\"}"
who="${3:-}"
weight="${4:-0.01}"
[ -f "$capture" ] || { echo "!! no such file: $capture" >&2; exit 1; }

# A dump from the local collector can carry the sender's public address in the
# WebRTC section (the hosted page redacts before download). The converter never
# reads it, but a file on your disk should not either: redact in place first.
tmp="$(mktemp -t capture).json"
cargo run -q -p fury-detect -- redact "$capture" "$tmp" >/dev/null 2>&1 || cp "$capture" "$tmp"

persona="$(mktemp -t persona).json"
if ! cargo run -q -p fury-detect -- persona "$tmp" --weight "$weight" > "$persona"; then
  echo "!! the converter refused this dump — usually it came from Fury or another modified browser rather than ordinary Chrome" >&2
  exit 1
fi

id=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "$persona")
if [ -e "shared/personas/contributed/$id.json" ] || grep -q "id: \"$id\"" shared-rs/src/catalogue.rs; then
  # Two people with the same machine: the second file is still that machine,
  # named apart. personas-check refuses a reused id.
  id="$id-$(date +%Y%m%d)"
fi

python3 - "$persona" "$id" "$who" "$machine" <<'PY'
import json, sys
src, id, who, machine = sys.argv[1:]
p = json.load(open(src))
p["id"] = id
if who: p["contributed_by"] = who
p["machine"] = machine
ordered = {k: p[k] for k in ("id", "weight", "source", "contributed_by", "machine") if k in p}
ordered.update({k: v for k, v in p.items() if k not in ordered})
out = f"shared/personas/contributed/{id}.json"
with open(out, "w", encoding="utf-8") as f:
    json.dump(ordered, f, indent=2, ensure_ascii=False); f.write("\n")
print(f"   wrote {out}")
PY
rm -f "$tmp" "$persona"

echo "== personas-check"
cargo run -q -p fury-detect -- personas-check
echo
echo "Next: git add shared/personas/contributed/$id.json && git commit"
