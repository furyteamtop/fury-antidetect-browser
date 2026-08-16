#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# How far along is the Windows build, in percent and in minutes.
#
#     tools/build-status.sh              # answer once
#     tools/build-status.sh --watch      # keep answering, every 30 seconds
#
# Run it from a Mac while the build runs on the Windows server. It asks over
# SSH and changes nothing.
#
# WHY IT EXISTS. A Chromium build takes eight hours and, until this file, the
# only way to know where it was in that eight hours was to ask me. That is a bad
# answer to give somebody who has other things to do, and it was bad for a
# stupid reason: ninja prints "[4213/57740]" on every step, every caller here
# piped it through `tail`, and `tail` shows nothing until the pipe closes. The
# number was being produced continuously and thrown away.
#
# WHAT THE ESTIMATE IS WORTH. The remaining time assumes the rest of the build
# goes at the average pace of what has already been done, and it does not: the
# steps are not the same size. Blink takes 30-40 seconds a file, the components
# take 5-15, and the last step of all -- linking chrome.dll -- is one step that
# can take an hour on its own. So expect the estimate to be optimistic in the
# middle and to sit at 99% for a while at the end. It is honest about the pace
# so far; it cannot be honest about a pace it has not seen.
set -euo pipefail

SERVER="${FURY_BUILD_SERVER:-user@your-windows-box}"
KEY="${FURY_BUILD_KEY:-$HOME/.ssh/fury_winbuild}"
TARGET="${1:-windows-x64}"
case "$TARGET" in --watch) TARGET="windows-x64" ;; esac

watch=0
for a in "$@"; do [ "$a" = "--watch" ] && watch=1; done

BASH_EXE='"C:\Program Files\Git\bin\bash.exe"'

# The question is asked by a small script COPIED to the server and run there,
# rather than piped into ssh.
#
# Piping is the obvious way and it does not work here: the remote shell is Git
# Bash launched through Windows OpenSSH, `bash /dev/stdin` fails with "No such
# file or directory", and every quoting fix makes the next layer worse -- the
# command crosses ssh, cmd.exe and bash, each with its own idea of a backslash.
# A file crosses all three unchanged.
remote() {
  local tmp
  tmp=$(mktemp -t fury-status)
  cat > "$tmp" <<'REMOTE'
out=/c/fury/core/src/out/TARGET_PLACEHOLDER.noindex
log="$out/build-progress.log"
# The last progress line ninja printed. tr, because that line is rewritten in
# place with a carriage return rather than a newline -- the whole build is one
# very long line until it finishes.
last=$( { tr '\r' '\n' < "$log" || true; } 2>/dev/null | grep -o '^\[[0-9]*/[0-9]*\]' | tail -1 || true)
echo "steps=${last}"
# Fallback for a build started before build-progress.log existed.
#
# .ninja_log records one line per finished step and KEEPS the lines from previous
# builds in the same directory, so its length is not this build's progress. The
# first field is milliseconds since the start of the ninja run that wrote it, so
# a restart shows up as that number dropping.
#
# Compared against the MAXIMUM seen so far and not against the previous line,
# which was the first attempt and was wrong: steps finish out of the order they
# started, so the start times are not monotonic and a plain "smaller than the
# last one" fires on almost every line. A real restart is a fall of more than
# five minutes below the high-water mark; nothing inside one run does that.
#
# The total cannot be recovered this way at all -- a log of finished steps says
# nothing about steps not taken -- so it comes from .fury-total, written by
# whoever last saw a full build of this target finish.
echo "fallback_done=$(awk '{ if ($1 + 300000 < max) { start = NR - 1; max = $1 } if ($1 > max) max = $1 } END { print NR - start }' "$out/.ninja_log" 2>/dev/null || echo 0)"
echo "fallback_total=$(cat "$out/.fury-total" 2>/dev/null || echo 0)"
# When this build started. args.gn is written by gn gen, the first thing build.sh
# does, and it is rewritten every run -- so unlike .ninja_log it never carries a
# time from the previous build.
echo "started=$(stat -c %Y "$out/args.gn" 2>/dev/null || echo 0)"
echo "now=$(date +%s)"
echo "running=$(ps -W 2>/dev/null | grep -c clang-cl || echo 0)"
echo "failed=$(grep -c 'ninja: build stopped' "$log" 2>/dev/null || echo 0)"
REMOTE
  sed -i '' "s/TARGET_PLACEHOLDER/$TARGET/" "$tmp" 2>/dev/null \
    || sed -i "s/TARGET_PLACEHOLDER/$TARGET/" "$tmp"
  scp -i "$KEY" -o BatchMode=yes -q "$tmp" "$SERVER:C:/Users/root/fury-status.sh"
  rm -f "$tmp"
  ssh -i "$KEY" -o BatchMode=yes -o ConnectTimeout=20 "$SERVER" \
    "$BASH_EXE -lc \"bash /c/Users/root/fury-status.sh\""
}

show() {
  local raw steps started now running failed n total pct elapsed rate left bar filled i
  raw=$(remote) || { echo "the build server did not answer"; return 1; }
  steps=$(printf '%s\n' "$raw" | sed -n 's/^steps=//p')
  started=$(printf '%s\n' "$raw" | sed -n 's/^started=//p')
  now=$(printf '%s\n' "$raw" | sed -n 's/^now=//p')
  running=$(printf '%s\n' "$raw" | sed -n 's/^running=//p')
  failed=$(printf '%s\n' "$raw" | sed -n 's/^failed=//p')

  if [ -n "$steps" ]; then
    n=${steps#[}; n=${n%%/*}
    total=${steps##*/}; total=${total%]}
  else
    n=$(printf '%s\n' "$raw" | sed -n 's/^fallback_done=//p')
    total=$(printf '%s\n' "$raw" | sed -n 's/^fallback_total=//p')
    if [ -z "${n:-}" ] || [ -z "${total:-}" ] || [ "${total:-0}" -eq 0 ]; then
      echo "  no progress to read yet -- gn gen is still running"
      return 0
    fi
    echo "  (counted from the step log; the total is remembered from the last full build)"
  fi
  elapsed=$(( now - started ))
  pct=$(( n * 100 / total ))

  # A bar, because a number alone does not show that it moved.
  filled=$(( pct * 40 / 100 ))
  bar=""
  for ((i = 0; i < 40; i++)); do
    if [ "$i" -lt "$filled" ]; then bar="$bar#"; else bar="$bar."; fi
  done

  printf '\n  [%s] %d%%\n' "$bar" "$pct"
  printf '  %s of %s steps\n' "$n" "$total"
  printf '  running for %s\n' "$(hms $elapsed)"

  if [ "$n" -gt 100 ] && [ "$elapsed" -gt 60 ]; then
    # Integer arithmetic throughout: seconds per thousand steps, so the numbers
    # stay whole without bc, which is not on every machine.
    rate=$(( elapsed * 1000 / n ))
    left=$(( (total - n) * rate / 1000 ))
    printf '  roughly %s left, at the pace so far\n' "$(hms $left)"
  fi

  if [ "${failed:-0}" -gt 0 ]; then
    printf '  STOPPED -- the log has a failure in it\n'
  elif [ "${running:-0}" -gt 0 ]; then
    printf '  %s compiler processes are working\n' "$running"
  else
    printf '  no compiler is running -- finished, or linking (one long step)\n'
  fi
  echo
}

hms() {
  local s=$1
  if [ "$s" -lt 60 ]; then printf '%d sec' "$s"
  elif [ "$s" -lt 3600 ]; then printf '%d min' $(( s / 60 ))
  else printf '%d h %02d min' $(( s / 3600 )) $(( (s % 3600) / 60 ))
  fi
}

if [ "$watch" = "1" ]; then
  while true; do show; sleep 30; done
else
  show
fi
