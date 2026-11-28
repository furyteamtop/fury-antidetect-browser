#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# Team mode, end to end, on one computer pretending to be two.
#
#     tools/team-e2e/run.sh
#
# Brings up a throwaway PostgreSQL, fury-server and two agents (machine A and
# machine B, each with its own FURY_HOME), runs run.mjs against them, and takes
# everything down again. Nothing touches the real installation: the database,
# the bundles, both profile directories and both sockets live in a scratch
# directory that is deleted at the end (kept with FURY_E2E_KEEP=1).
#
# Needs: PostgreSQL's initdb/pg_ctl (on PATH, or Postgres.app in ~/Applications
# or /Applications), node 22+, a built core (FURY_CORE, or the one the installed
# Fury downloaded), and `cargo build -p fury-server -p fury-agent` done.
# The binaries are taken from target/debug unless FURY_E2E_BIN says otherwise --
# which is how the same run is pointed at an older build to compare.
set -euo pipefail

repo="$(cd "$(dirname "$0")/../.." && pwd)"
bin="${FURY_E2E_BIN:-$repo/target/debug}"

if [[ -z "${FURY_CORE:-}" ]]; then
  for c in "$HOME/Library/Application Support/Fury/core.noindex/Fury.app/Contents/MacOS/Fury" \
           "$HOME/Library/Application Support/Fury/core.bundle/Fury.app/Contents/MacOS/Fury" \
           "${LOCALAPPDATA:-/nonexistent}/Fury/core/fury.exe"; do
    [[ -x "$c" ]] && export FURY_CORE="$c" && break
  done
fi
[[ -n "${FURY_CORE:-}" ]] || { echo "no core: set FURY_CORE" >&2; exit 1; }

pgbin=""
if command -v initdb >/dev/null; then pgbin="$(dirname "$(command -v initdb)")"; fi
for p in "$HOME/Applications/Postgres.app" "/Applications/Postgres.app"; do
  [[ -z "$pgbin" && -d "$p" ]] && pgbin="$(ls -d "$p"/Contents/Versions/*/bin | sort -V | tail -1)"
done
[[ -n "$pgbin" ]] || { echo "no PostgreSQL: put initdb on PATH" >&2; exit 1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/fury-e2e.XXXXXX")"
# Unix socket paths must fit in sun_path (104 bytes on macOS); TMPDIR there is
# long enough to break that, so the sockets go somewhere short.
sockdir="$(mktemp -d /tmp/fe2e.XXXX)"
pgport=$((55000 + RANDOM % 900))
srvport=$((18000 + RANDOM % 900))
pids=()

cleanup() {
  for pid in "${pids[@]+"${pids[@]}"}"; do kill "$pid" 2>/dev/null || true; done
  # The agents close their browsers when they stop; give them a moment.
  sleep 2
  "$pgbin/pg_ctl" -D "$work/pg" stop -m fast >/dev/null 2>&1 || true
  if [[ "${FURY_E2E_KEEP:-}" == "1" ]]; then
    echo "kept: $work"
  else
    rm -rf "$work"
  fi
  rm -rf "$sockdir"
}
trap cleanup EXIT

# A locale PostgreSQL cannot use (a Russian macOS sets one) stops initdb, and
# the postmaster then refuses to start as "multithreaded during startup".
LC_ALL=C LANG=C "$pgbin/initdb" -D "$work/pg" -U fury --auth=trust >/dev/null
LC_ALL=C LANG=C "$pgbin/pg_ctl" -D "$work/pg" -o "-p $pgport -k ''" -l "$work/pg.log" start >/dev/null
"$pgbin/psql" -h 127.0.0.1 -p "$pgport" -U fury -d postgres -q \
  -c "create role app login" -c "create database fury owner app"

mkdir -p "$work/bundles"
DATABASE_URL="postgres://app@127.0.0.1:$pgport/fury" FURY_BUNDLE_DIR="$work/bundles" \
  BIND="127.0.0.1:$srvport" FURY_OPEN_SIGNUP=1 RUST_LOG=info \
  "$bin/fury-server" >"$work/server.log" 2>&1 &
pids+=($!)

for m in A B; do
  mkdir -p "$work/$m"
  FURY_HOME="$work/$m" FURY_SOCKET="$sockdir/$m.sock" RUST_LOG=info \
    "$bin/fury-agent" serve >"$work/$m/stdout.log" 2>&1 &
  pids+=($!)
done

for _ in $(seq 1 50); do
  curl -sf "http://127.0.0.1:$srvport/healthz" >/dev/null && [[ -S "$sockdir/A.sock" && -S "$sockdir/B.sock" ]] && break
  sleep 0.2
done

FURY_E2E_SERVER="http://127.0.0.1:$srvport" \
FURY_E2E_SOCK_A="$sockdir/A.sock" FURY_E2E_SOCK_B="$sockdir/B.sock" \
FURY_E2E_HOME_A="$work/A" FURY_E2E_HOME_B="$work/B" \
FURY_E2E_BUNDLES="$work/bundles" \
  node "$repo/tools/team-e2e/run.mjs"
