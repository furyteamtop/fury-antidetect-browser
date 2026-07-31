#!/usr/bin/env bash
#
# Copy the server's source to a box and run the installer. Run from a laptop.
#
#   ./deploy/push.sh root@203.0.113.7 203-0-113-7.sslip.io [~/.ssh/id_ed25519]
#
# Only the two crates the server is built from travel. The rest of the
# repository is a 39 GB Chromium checkout and a desktop application, neither of
# which has any business on a server.
set -euo pipefail

TARGET="${1:?usage: push.sh user@host hostname [ssh-key]}"
HOSTNAME="${2:?usage: push.sh user@host hostname [ssh-key]}"
KEY="${3:-}"

cd "$(dirname "${BASH_SOURCE[0]}")/.."
SSH=(ssh -o StrictHostKeyChecking=accept-new)
[ -n "$KEY" ] && SSH+=(-i "$KEY")

echo "==> preparing $TARGET"
"${SSH[@]}" "$TARGET" 'mkdir -p /opt/fury/src'

echo "==> copying source"
rsync -az --delete \
    -e "$(printf '%q ' "${SSH[@]}")" \
    --exclude 'target/' \
    ./server ./shared-rs ./deploy \
    "$TARGET:/opt/fury/src/"

echo "==> installing"
# -t so apt and rustup have a terminal; without it rustup writes progress bars
# into a pipe and some apt prompts hang forever rather than defaulting.
"${SSH[@]}" -t "$TARGET" \
    "chmod +x /opt/fury/src/deploy/server-install.sh && HOSTNAME='$HOSTNAME' /opt/fury/src/deploy/server-install.sh"
