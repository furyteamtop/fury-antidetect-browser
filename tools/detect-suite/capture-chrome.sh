#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

# Capture a fingerprint baseline from a real browser with no clicking.
#
#   tools/detect-suite/capture-chrome.sh [name] [browser]
#
#   name     output file in baselines/ (default: derived from the browser build)
#   browser  path to the .app or binary (default: Google Chrome)
#
# Launches the browser with a throwaway profile pointed at probe.html?auto=<name>,
# waits for the collector to write the file, then closes it.
#
# A throwaway profile is deliberate: a reference baseline must describe a clean
# browser, not one shaped by whatever extensions the operator has installed.
# It also means this never touches the real profile.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PORT="${PORT:-8731}"
# 127.0.0.1, never "localhost": on macOS localhost may resolve to ::1 first, and
# a v4-only collector then gets no request at all — which is exactly the failure
# this script was written to eliminate.
HOST="127.0.0.1"

NAME="${1:-}"
BROWSER="${2:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"

if [ ! -x "$BROWSER" ]; then
  echo "!! Browser not found or not executable: $BROWSER" >&2
  echo "!! Pass the path explicitly:" >&2
  echo "     $0 my-baseline '/Applications/Brave Browser.app/Contents/MacOS/Brave Browser'" >&2
  exit 1
fi

# --- collector must be up ----------------------------------------------------
if ! curl -sS -o /dev/null --max-time 3 "http://$HOST:$PORT/probe.html"; then
  echo "==> Collector not responding on $HOST:$PORT, starting it"
  python3 "$HERE/collector.py" >"$HERE/collector.log" 2>&1 &
  COLLECTOR_PID=$!
  trap 'kill $COLLECTOR_PID 2>/dev/null || true' EXIT
  for _ in $(seq 1 20); do
    curl -sS -o /dev/null --max-time 1 "http://$HOST:$PORT/probe.html" && break
    sleep 0.5
  done
fi

# --- name --------------------------------------------------------------------
if [ -z "$NAME" ]; then
  raw="$("$BROWSER" --version 2>/dev/null || echo unknown)"
  major="$(printf '%s' "$raw" | grep -oE '[0-9]+' | head -1)"
  os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  [ "$os" = "darwin" ] && os="macos"
  NAME="chrome-${major:-unknown}-${os}-$(uname -m)"
  echo "==> Browser reports: $raw"
fi

TARGET="$HERE/baselines/$NAME.json"
rm -f "$TARGET"
PROFILE="$(mktemp -d)"
cleanup_profile() { sleep 1; rm -rf "$PROFILE" 2>/dev/null || true; }
trap cleanup_profile EXIT

echo "==> Capturing $NAME"

# --no-first-run / --no-default-browser-check keep the throwaway profile from
# showing dialogs that would block the page. Nothing here alters the fingerprint
# the probe measures.
"$BROWSER" \
  --user-data-dir="$PROFILE" \
  --no-first-run \
  --no-default-browser-check \
  --disable-sync \
  --new-window \
  "http://$HOST:$PORT/probe.html?auto=$NAME" \
  >/dev/null 2>&1 &
BROWSER_PID=$!

# --- wait for the file -------------------------------------------------------
for _ in $(seq 1 60); do
  if [ -s "$TARGET" ]; then
    kill "$BROWSER_PID" 2>/dev/null || true
    wait "$BROWSER_PID" 2>/dev/null || true
    echo "==> Saved $(basename "$TARGET") ($(wc -c <"$TARGET" | tr -d ' ') bytes)"
    exit 0
  fi
  sleep 1
done

kill "$BROWSER_PID" 2>/dev/null || true
echo "!! Timed out after 60s. Check $HERE/collector.log — every request is logged there." >&2
exit 1
