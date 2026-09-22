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
# WHERE IT LANDS. /var/www/fury on the host, served by Caddy. The Caddyfile
# block is installed by this script when it is missing, and left alone when
# it is there: the API block above it is the live server and is not this
# script's to rewrite.
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

# The site block, added once. `caddy validate` before the reload, because a
# Caddyfile that does not parse takes the API down with it, and the API is
# what somebody's browser profiles sync through.
echo "==> Caddy"
ssh -i "$key" "$target" "bash -s" <<'REMOTE'
set -euo pipefail
conf=/etc/caddy/Caddyfile
if grep -q "root \* /var/www/fury" "$conf"; then
  echo "   the site block is already there"
else
  cp "$conf" "$conf.before-landing"
  cat >> "$conf" <<'BLOCK'

# The landing page. Static files, no proxy: it must not be able to reach the
# API by accident, and the API block above answers on its own name.
#
# sslip.io name first because it resolves to this machine today — the page is
# verifiable before the domain's A record moves. The apex and www are listed
# too; until they point here Caddy simply keeps failing to get a certificate
# for them, and serves the rest.
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
  echo "   added"
fi
caddy validate --config "$conf" --adapter caddyfile >/dev/null
systemctl reload caddy
echo "   validated and reloaded"
REMOTE

echo
echo "live:  https://site.204-168-178-23.sslip.io/"
echo "       https://furybrowser.dev/   (once the A record points at this host)"
