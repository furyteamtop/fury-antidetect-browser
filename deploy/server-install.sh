#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

#
# Install a Fury server on a fresh Ubuntu box. Run as root, on the server.
#
# Native rather than Docker, deliberately: this runs on the smallest VPS anyone
# will rent, and a daemon plus an image layer to hold one 12 MB binary and one
# Postgres is overhead with nothing to show for it. It also means the logs are
# in journalctl and the database is where a Postgres administrator expects it,
# which matters the day something is wrong at an hour nobody wants to debug
# container networking.
#
# Idempotent: safe to re-run to upgrade. Everything it creates is named `fury`.
#
#   HOSTNAME=fury.example.com ./server-install.sh
#
# HOSTNAME is what the certificate is issued for and what operators type into
# the app. With no domain, pass the sslip.io form of the address — it resolves
# to the address itself and Let's Encrypt will issue for it:
#
#   HOSTNAME=203-0-113-7.sslip.io ./server-install.sh
set -euo pipefail

HOSTNAME="${HOSTNAME:?set HOSTNAME to the name this server answers to}"
SRC="${SRC:-/opt/fury/src}"
say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

# Swap, on a box that has none.
#
# The smallest VPS worth renting has 4 GB, and linking this server with thin LTO
# on two cores comes close enough to that to fail on a bad day — as an OOM kill
# in the middle of a build, which reads as a mysterious hang rather than as "out
# of memory". Two gigabytes of file is cheap insurance, and Postgres is happier
# with it there afterwards.
if [ "$(swapon --show --noheadings | wc -l)" -eq 0 ]; then
    say "Swap"
    fallocate -l 2G /swapfile || dd if=/dev/zero of=/swapfile bs=1M count=2048
    chmod 600 /swapfile
    mkswap /swapfile >/dev/null
    swapon /swapfile
    grep -q '^/swapfile' /etc/fstab || echo '/swapfile none swap sw 0 0' >> /etc/fstab
fi

say "Packages"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq \
    postgresql postgresql-contrib \
    `# contrib carries citext, which the schema uses for case-insensitive` \
    `# email. pgcrypto used to be needed too and is not: gen_random_uuid()` \
    `# has been in core since PostgreSQL 13, and asking for the extension` \
    `# made the migration fail outright on any build without contrib.` \
    build-essential pkg-config libssl-dev \
    curl git ufw ca-certificates debian-keyring debian-archive-keyring apt-transport-https

# Caddy, for TLS that renews itself. A self-hosted product whose certificate
# expires on a Sunday is a product nobody can work with on Sunday.
if ! command -v caddy >/dev/null; then
    say "Caddy"
    curl -fsSL https://dl.cloudsmith.io/public/caddy/stable/gpg.key \
        | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
    curl -fsSL https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt \
        | tee /etc/apt/sources.list.d/caddy-stable.list >/dev/null
    apt-get update -qq && apt-get install -y -qq caddy
fi

say "User and directories"
id fury >/dev/null 2>&1 || useradd --system --home /var/lib/fury --shell /usr/sbin/nologin fury
# The bundles are the profiles: cookie jars for live accounts, encrypted, but
# still the thing a competitor would most like. Nothing else on the box reads
# this directory.
install -d -o fury -g fury -m 0700 /var/lib/fury /var/lib/fury/bundles
install -d -o root -g fury -m 0750 /etc/fury

say "Database"
DB_PASS_FILE=/etc/fury/db-password
if [ ! -f "$DB_PASS_FILE" ]; then
    # Generated here and never printed. A password in the installer's output is
    # a password in somebody's terminal scrollback and then in a screenshot.
    head -c 24 /dev/urandom | base64 | tr -d '/+=' > "$DB_PASS_FILE"
    chmod 0640 "$DB_PASS_FILE"; chown root:fury "$DB_PASS_FILE"
fi
DB_PASS="$(cat "$DB_PASS_FILE")"

sudo -u postgres psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='fury'" | grep -q 1 \
    || sudo -u postgres psql -qc "CREATE ROLE fury LOGIN PASSWORD '${DB_PASS}'"
sudo -u postgres psql -qc "ALTER ROLE fury PASSWORD '${DB_PASS}'"
sudo -u postgres psql -tAc "SELECT 1 FROM pg_database WHERE datname='fury'" | grep -q 1 \
    || sudo -u postgres createdb -O fury fury
# citext is used for email addresses, so that Bogdan@ and bogdan@ are one
# account rather than two people who cannot see each other's work.
sudo -u postgres psql -q -d fury -c "CREATE EXTENSION IF NOT EXISTS citext"

say "Rust"
if ! [ -x /root/.cargo/bin/cargo ]; then
    curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
fi
export PATH="/root/.cargo/bin:$PATH"

say "Build"
# Only the two crates the server needs. The workspace also contains the desktop
# shell, which would drag in GTK and WebKit — a build dependency tree for a
# graphical application, on a machine with no display.
cat > "$SRC/Cargo.toml" <<'WORKSPACE'
[workspace]
resolver = "2"
members = ["server", "shared-rs"]

[workspace.package]
version = "0.0.1"
edition = "2021"
rust-version = "1.80"
license = "AGPL-3.0-or-later"

[workspace.dependencies]
anyhow = "1"
thiserror = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
uuid = { version = "1", features = ["v7", "serde"] }
time = { version = "0.3", features = ["serde", "formatting"] }
sha2 = "0.10"
rand = "0.8"

[profile.release]
lto = "thin"
codegen-units = 1
strip = true
WORKSPACE

cd "$SRC"
cargo build --release -p fury-server
install -o root -g root -m 0755 target/release/fury-server /usr/local/bin/fury-server

say "Configuration"
cat > /etc/fury/fury.env <<ENV
DATABASE_URL=postgres://fury:${DB_PASS}@127.0.0.1:5432/fury
# Loopback only. Caddy terminates TLS and forwards; nothing reaches the
# application except through it, so a misconfigured firewall cannot expose
# plain HTTP carrying session tokens.
BIND=127.0.0.1:8901
FURY_BUNDLE_DIR=/var/lib/fury/bundles
RUST_LOG=fury_server=info,tower_http=warn
# Whether a stranger who finds this address may make themselves an account.
#
# Off. A self-hosted server is somebody's own machine, and "yes" is not a
# default anyone should inherit by installing something. Set it to 1 to run a
# server other people can join, then restart: systemctl restart fury-server.
#
# It is less alarming than it sounds — every sign-up creates its OWN
# organisation, and an organisation is the boundary every query already filters
# on, so a stranger cannot address anything of yours. What they cost you is rows.
#
# Somebody who signs up this way is the owner of their organisation and can run
# it: invite colleagues from the application, hand them the organisation key,
# and grant per-project access. That is the whole team feature, and it is theirs
# as much as yours — the two organisations simply never meet.
FURY_OPEN_SIGNUP=0

# What one organisation may take.
#
# Empty means no limit, which is right for a box you run for your own team: a
# quota somebody hits at nine on a Monday is worse than no quota.
#
# They exist for the other case. With FURY_OPEN_SIGNUP=1 the isolation between
# organisations is total and the FAIRNESS is not — nobody can read anybody's
# data, and one stranger with a loop can still take the whole disk. Set all
# three before you open sign-ups.
#
#   FURY_MAX_ORGS=50                  organisations on this server, total
#   FURY_MAX_PROFILES_PER_ORG=200     profiles one organisation may hold
#   FURY_MAX_STORAGE_PER_ORG=5368709120   bundle bytes per organisation (5 GB)
#
FURY_MAX_ORGS=
FURY_MAX_PROFILES_PER_ORG=
FURY_MAX_STORAGE_PER_ORG=
ENV
chmod 0640 /etc/fury/fury.env; chown root:fury /etc/fury/fury.env

cat > /etc/systemd/system/fury-server.service <<'UNIT'
[Unit]
Description=Fury server
After=network-online.target postgresql.service
Wants=network-online.target
Requires=postgresql.service

[Service]
Type=simple
User=fury
Group=fury
EnvironmentFile=/etc/fury/fury.env
ExecStart=/usr/local/bin/fury-server
Restart=on-failure
RestartSec=5

# The process holds every team's encrypted bundles and a database password. It
# has no reason to read anything else on the machine, so it cannot.
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
PrivateDevices=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
RestrictNamespaces=true
LockPersonality=true
MemoryDenyWriteExecute=true
ReadWritePaths=/var/lib/fury

[Install]
WantedBy=multi-user.target
UNIT

cat > /etc/caddy/Caddyfile <<CADDY
${HOSTNAME} {
    reverse_proxy 127.0.0.1:8901

    # A bundle is a whole browser profile. The default would refuse the upload
    # at the moment an operator closes a browser after a day's work.
    request_body {
        max_size 512MB
    }

    header {
        Strict-Transport-Security "max-age=31536000"
        X-Content-Type-Options "nosniff"
        -Server
    }
}
CADDY

say "Firewall"
ufw allow 22/tcp  >/dev/null
ufw allow 80/tcp  >/dev/null
ufw allow 443/tcp >/dev/null
ufw --force enable >/dev/null

say "Start"
systemctl daemon-reload
systemctl enable --now postgresql fury-server caddy
systemctl restart fury-server caddy

sleep 3
say "Check"
systemctl is-active --quiet fury-server && echo "  fury-server: running" || { echo "  fury-server FAILED"; journalctl -u fury-server -n 30 --no-pager; exit 1; }
systemctl is-active --quiet caddy && echo "  caddy: running" || echo "  caddy: NOT running"

# The app checks for exactly this pair before it will save an address, so the
# install is not finished until it answers the way the app expects.
code=$(curl -s -o /tmp/fury-probe -w '%{http_code}' "https://${HOSTNAME}/v1/me" || echo 000)
body=$(cat /tmp/fury-probe 2>/dev/null || true)
echo "  https://${HOSTNAME}/v1/me -> ${code} ${body}"
if [ "$code" = "401" ] && [ "$body" = '{"error":"unauthenticated"}' ]; then
    echo
    echo "  Ready. In the app, connect to: https://${HOSTNAME}"
else
    echo
    echo "  Not answering as expected yet. TLS can take a minute on first issue;"
    echo "  re-run the curl above. If it persists: journalctl -u caddy -n 50"
fi
