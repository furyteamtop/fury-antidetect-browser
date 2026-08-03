#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# The whole automation API, in curl.
#
# Read this one first. It is the shortest path from nothing to a browser you
# opened from a script, and every other example here is the same six calls with
# a library wrapped around them.
#
#   FURY_API_PORT=35000 fury-agent serve     # in another terminal
#   ./api.sh                                 # what have I got
#   ./api.sh <profile-id>                    # start it, look at it, stop it

set -euo pipefail

port="${FURY_API_PORT:-35000}"
base="http://127.0.0.1:$port/v1"

# The token is read, never written down. It authorises every logged-in profile
# on this machine, and a script is the thing that gets pasted into an issue.
home="${FURY_HOME:-$HOME/Library/Application Support/Fury}"
token_file="$home/api-token"
if [ ! -f "$token_file" ]; then
  echo "!! no $token_file" >&2
  echo "   The agent creates it the first time it serves the API:" >&2
  echo "   FURY_API_PORT=$port fury-agent serve" >&2
  exit 1
fi
token=$(tr -d '[:space:]' < "$token_file")

api() {
  local method="$1" path="$2" body="${3:-}"
  local args=(-sS -X "$method" -H "Authorization: Bearer $token" "$base$path")
  [ -n "$body" ] && args+=(-H 'Content-Type: application/json' -d "$body")
  curl "${args[@]}"
}

# jq is deliberately not assumed — python3 is on every macOS and every Linux
# that matters, and jq is on neither by default.

echo "== status"
if ! status=$(api GET /status 2>&1); then
  echo "!! could not reach the agent on port $port" >&2
  echo "   Start it with: FURY_API_PORT=$port fury-agent serve" >&2
  exit 1
fi
echo "$status" | python3 -m json.tool

profile_id="${1:-}"

if [ -z "$profile_id" ]; then
  echo
  echo "== profiles"
  api GET /profiles | python3 -c '
import json, sys
data = json.load(sys.stdin).get("data", [])
if not data:
    print("  none yet — make one in the application")
for p in data:
    running = "running" if p.get("running") else "stopped"
    print(f'"'"'  {p["id"]}  {p["name"]:<24} {running}'"'"')
'
  echo
  echo "Start one:  $0 <profile-id>"
  exit 0
fi

echo
echo "== starting $profile_id with a debugging port"
# cdp:true is what opens the port. Without it the profile launches perfectly
# well and there is nothing for a driver to connect to — which is the right
# default, because a debugging port nobody asked for is a way into a browser
# nobody is watching.
started=$(api POST /profiles/start "{\"id\":\"$profile_id\",\"cdp\":true}")
echo "$started" | python3 -m json.tool

ws=$(printf '%s' "$started" | python3 -c '
import json, sys
print(json.load(sys.stdin).get("data", {}).get("ws_endpoint", ""))
')

if [ -z "$ws" ]; then
  # Not an error on its own: a team server can forbid CDP for a role, in which
  # case the browser starts and there is deliberately no endpoint.
  echo
  echo "no ws_endpoint — the browser is running, but CDP was not granted."
  echo "Either the role forbids it, or the core is older than the flag."
else
  echo
  echo "DevTools: $ws"
  echo
  echo "== asking the browser what it thinks it is"
  # Straight HTTP against the DevTools port: no library, no dependencies. This
  # is the same information a page would see, from outside the page.
  http_base="http://127.0.0.1:$(printf '%s' "$ws" | sed 's|ws://127.0.0.1:||; s|/.*||')"
  curl -sS "$http_base/json/version" | python3 -m json.tool || true
fi

echo
read -r -p "press enter to stop the profile "
echo "== stopping"
api POST /profiles/stop "{\"id\":\"$profile_id\"}" | python3 -m json.tool
