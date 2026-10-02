#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# Put the landing page on the server that already serves the team API.
#
#     site/deploy.sh [user@host]
#
# Builds first, always. A deploy that ships whatever happens to be in dist/
# is a deploy that can publish a page built from a tree three commits old,
# and the page prints the commit it was built from — so it would say so, in
# public, at the bottom of the page.
#
# WHERE IT LANDS. /var/www/fury on the host, served by Caddy from its own
# file, /etc/caddy/conf.d/landing.caddy, which this script rewrites every
# time. Not the Caddyfile: that one belongs to deploy/server-install.sh, which
# rewrites it on every server update, and on 01.10.2026 took a block this
# script had appended there with it.
#
# The page is static, so there is nothing to restart and no state to migrate.
# rsync --delete keeps the directory equal to dist/ rather than accumulating
# files nobody serves any more.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
target="${1:-root@204.168.178.23}"
key="${FURY_SSH_KEY:-$HOME/.ssh/fury_server}"
root="/var/www/fury"

echo "==> building"
python3 "$here/build.py"

echo "==> copying to $target:$root"
ssh -i "$key" "$target" "mkdir -p $root"
rsync -az --delete -e "ssh -i $key" "$here/dist/" "$target:$root/"

# `caddy validate` before the reload, because a Caddyfile that does not parse
# takes the API down with it, and the API is what somebody's browser profiles
# sync through.
echo "==> Caddy"
ssh -i "$key" "$target" "bash -s" <<'REMOTE'
set -euo pipefail
conf=/etc/caddy/Caddyfile
if ! grep -q '^import /etc/caddy/conf.d/' "$conf"; then
  echo "   $conf does not import conf.d: re-run deploy/server-install.sh first" >&2
  exit 1
fi
mkdir -p /etc/caddy/conf.d
cat > /etc/caddy/conf.d/landing.caddy <<'BLOCK'
# The landing page, written by site/deploy.sh. Static files, no proxy: it must
# not be able to reach the API by accident, and the API block answers on its
# own name.
furybrowser.dev, www.furybrowser.dev, site.204-168-178-23.sslip.io {
    root * /var/www/fury
    file_server
    encode gzip zstd

    header {
        Strict-Transport-Security "max-age=31536000"
        X-Content-Type-Options "nosniff"
        Referrer-Policy "strict-origin-when-cross-origin"
        -Server
    }

    header /*.png Cache-Control "public, max-age=604800"
    header /index.html Cache-Control "no-cache"
}
BLOCK
caddy validate --config "$conf" --adapter caddyfile >/dev/null
systemctl reload caddy
echo "   validated and reloaded"
REMOTE

echo
echo "live:  https://site.204-168-178-23.sslip.io/"
echo "       https://furybrowser.dev/"
