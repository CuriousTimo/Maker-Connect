#!/usr/bin/env bash
# Installs the Maker Connect hub on Debian 13 (a Proxmox LXC from scripts/proxmox-hub.sh, or any
# Debian 13 machine), as root. Run it again to update: Debian's packages, and the hub to the newest
# signed hub-v… release. Safe to run twice.
#
#   bash -c "$(curl -fsSL https://raw.githubusercontent.com/CuriousTimo/Maker-Connect/main/scripts/hub-install.sh)"
#
# What it puts in place (docs/pi-hub.md, with the database native instead of in a container):
# - PostgreSQL 17 from PostgreSQL's apt repository and TimescaleDB 2.32.0 from Timescale's, both
#   held at that version; listening on 127.0.0.1 only.
# - The hub's roles and restore template (scripts/db-roles.sql of the hub's release) and its
#   database maker_connect, owned by maker_connect_owner, with TimescaleDB in it.
# - The user maker-connect, /var/lib/maker-connect/ (the hub, spoolmandb/, its files and dumps),
#   /etc/maker-connect/hub.env (0600, DATABASE_URL with a password made here), and the systemd unit
#   maker-connect-hub.
# - The hub from the newest hub-v… release, taken only when the release's SHA256SUMS.sig (Ed25519
#   over "<tag>\n<SHA256SUMS>", the public key of crates/update's RELEASE_KEY) holds and the
#   archive's sum is on that list.
set -euo pipefail

REPO=CuriousTimo/Maker-Connect
PG=17
TIMESCALE=2.32.0
HOME_DIR=/var/lib/maker-connect
ENV_FILE=/etc/maker-connect/hub.env
UNIT=/etc/systemd/system/maker-connect-hub.service
BACKUPS=/var/backups/maker-connect
# crates/update/src/lib.rs RELEASE_KEY, as an Ed25519 public key in PEM (SubjectPublicKeyInfo).
RELEASE_KEY_PEM='-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAcFM4BpEuIFw/FGFGh7lMATHIUr7bQ7GyZUDO9cNYglE=
-----END PUBLIC KEY-----'

say() { printf '\e[1;32m==\e[0m %s\n' "$*"; }
fail() { printf '\e[1;31mhub-install:\e[0m %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" = 0 ] || fail "run it as root"
. /etc/os-release
[ "${ID:-}" = debian ] && [ "${VERSION_CODENAME:-}" = trixie ] || fail "this is for Debian 13 (trixie), not ${PRETTY_NAME:-this system}"
case "$(uname -m)" in
x86_64) TARGET=linux-x86_64 ;;
aarch64) TARGET=linux-aarch64 ;;
*) fail "no hub release for $(uname -m)" ;;
esac

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# --- Debian's packages, with PostgreSQL's and Timescale's repositories -----------------------
say "packages"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq ca-certificates curl gnupg jq openssl >/dev/null
install -d -m 0755 /etc/apt/keyrings
if [ ! -f /etc/apt/sources.list.d/pgdg.sources ]; then
    curl -fsSL https://www.postgresql.org/media/keys/ACCC4CF8.asc | gpg --dearmor -o /etc/apt/keyrings/pgdg.gpg
    cat >/etc/apt/sources.list.d/pgdg.sources <<EOF
Types: deb
URIs: https://apt.postgresql.org/pub/repos/apt
Suites: trixie-pgdg
Components: main
Signed-By: /etc/apt/keyrings/pgdg.gpg
EOF
fi
if [ ! -f /etc/apt/sources.list.d/timescaledb.sources ]; then
    curl -fsSL https://packagecloud.io/timescale/timescaledb/gpgkey | gpg --dearmor -o /etc/apt/keyrings/timescaledb.gpg
    cat >/etc/apt/sources.list.d/timescaledb.sources <<EOF
Types: deb
URIs: https://packagecloud.io/timescale/timescaledb/debian
Suites: trixie
Components: main
Signed-By: /etc/apt/keyrings/timescaledb.gpg
EOF
fi
apt-get update -qq
# Held: an upgrade keeps them; a new TimescaleDB comes with a new version of this script.
TS_VERSION=$(apt-cache madison "timescaledb-2-postgresql-$PG" | awk -v v="$TIMESCALE~" 'index($3, v) == 1 {print $3; exit}')
[ -n "$TS_VERSION" ] || fail "TimescaleDB $TIMESCALE for PostgreSQL $PG is not in Timescale's repository"
apt-mark unhold "timescaledb-2-postgresql-$PG" "timescaledb-2-loader-postgresql-$PG" >/dev/null 2>&1 || true
apt-get install -y -qq --allow-downgrades "postgresql-$PG" "postgresql-client-$PG" \
    "timescaledb-2-postgresql-$PG=$TS_VERSION" "timescaledb-2-loader-postgresql-$PG=$TS_VERSION" >/dev/null
apt-mark hold "timescaledb-2-postgresql-$PG" "timescaledb-2-loader-postgresql-$PG" >/dev/null
apt-get -y -qq upgrade >/dev/null

# --- PostgreSQL: 127.0.0.1 only, TimescaleDB loaded ------------------------------------------
say "PostgreSQL $PG with TimescaleDB $TIMESCALE"
CONF_DIR=/etc/postgresql/$PG/main/conf.d
install -d -o postgres -g postgres "$CONF_DIR"
cat >"$WORK/maker-connect.conf" <<EOF
# Written by scripts/hub-install.sh (Maker Connect): changes here are overwritten by its next run.
listen_addresses = '127.0.0.1'
shared_preload_libraries = 'timescaledb'
EOF
systemctl enable -q "postgresql@$PG-main"
# Restarted only when its settings change: a restart restarts the hub too (Requires= in its unit).
if ! cmp -s "$WORK/maker-connect.conf" "$CONF_DIR/maker-connect.conf"; then
    install -o postgres -g postgres -m 0644 "$WORK/maker-connect.conf" "$CONF_DIR/maker-connect.conf"
    systemctl restart "postgresql@$PG-main"
else
    systemctl start "postgresql@$PG-main"
fi
psql_su() { runuser -u postgres -- psql -X -q -v ON_ERROR_STOP=1 "$@"; }
for _ in $(seq 30); do psql_su -c 'SELECT 1' >/dev/null 2>&1 && break; sleep 1; done
psql_su -c 'SELECT 1' >/dev/null || fail "PostgreSQL does not answer"

# --- The newest signed hub release ------------------------------------------------------------
say "the newest hub release"
curl -fsSL "https://api.github.com/repos/$REPO/releases?per_page=50" -o "$WORK/releases.json"
TAG=$(jq -r '[.[] | select(.draft | not) | select(.tag_name | startswith("hub-v"))][0].tag_name // empty' "$WORK/releases.json")
[ -n "$TAG" ] || fail "no hub-v… release on github.com/$REPO"
ARCHIVE=maker-connect-hub-$TARGET.tar.gz
for f in SHA256SUMS SHA256SUMS.sig "$ARCHIVE"; do
    curl -fsSL "https://github.com/$REPO/releases/download/$TAG/$f" -o "$WORK/$f"
done
printf '%s\n' "$RELEASE_KEY_PEM" >"$WORK/release-key.pem"
printf '%s\n' "$TAG" >"$WORK/signed"
cat "$WORK/SHA256SUMS" >>"$WORK/signed"
base64 -d "$WORK/SHA256SUMS.sig" >"$WORK/sig.bin" || fail "$TAG: SHA256SUMS.sig does not read"
openssl pkeyutl -verify -pubin -inkey "$WORK/release-key.pem" -rawin \
    -in "$WORK/signed" -sigfile "$WORK/sig.bin" >/dev/null 2>&1 ||
    fail "$TAG is not signed with Maker Connect's key: refused"
LISTED=$(awk -v f="$ARCHIVE" '{n=$2; sub(/^\*/, "", n)} n == f {print $1}' "$WORK/SHA256SUMS")
[ -n "$LISTED" ] || fail "$ARCHIVE is not in $TAG"
[ "$(sha256sum "$WORK/$ARCHIVE" | cut -d' ' -f1)" = "$LISTED" ] || fail "$ARCHIVE is not the file $TAG lists: refused"
tar xzf "$WORK/$ARCHIVE" -C "$WORK"
NEW=$WORK/maker-connect-hub-$TARGET
say "$TAG: signature and sum hold"

# --- The database's roles, template and database ----------------------------------------------
say "the database"
id maker-connect >/dev/null 2>&1 ||
    useradd --system --home "$HOME_DIR" --create-home --shell /usr/sbin/nologin maker-connect
install -d -m 0755 /etc/maker-connect
if [ -f "$ENV_FILE" ] && grep -q '^DATABASE_URL=' "$ENV_FILE"; then
    PASSWORD=$(sed -n 's|^DATABASE_URL=postgres://maker_connect_hub:\([^@]*\)@.*|\1|p' "$ENV_FILE")
fi
if [ -z "${PASSWORD:-}" ]; then
    PASSWORD=$(openssl rand -hex 24)
    install -m 0600 /dev/null "$ENV_FILE"
    printf 'DATABASE_URL=postgres://maker_connect_hub:%s@127.0.0.1:5432/maker_connect\n' "$PASSWORD" >"$ENV_FILE"
fi
chmod 0600 "$ENV_FILE"
curl -fsSL "https://raw.githubusercontent.com/$REPO/$TAG/scripts/db-roles.sql" -o "$WORK/db-roles.sql"
chmod 0644 "$WORK/db-roles.sql"
chmod 0755 "$WORK"
psql_su -v hub_password="$PASSWORD" -f "$WORK/db-roles.sql" >/dev/null
if [ "$(psql_su -tAc "SELECT 1 FROM pg_database WHERE datname = 'maker_connect'")" != 1 ]; then
    psql_su -c "CREATE DATABASE maker_connect OWNER maker_connect_owner"
fi
psql_su -d maker_connect -c "CREATE EXTENSION IF NOT EXISTS timescaledb" >/dev/null

# --- The hub ----------------------------------------------------------------------------------
say "the hub"
EXE=$HOME_DIR/maker-connect-hub
CHANGED=
if [ -f "$EXE" ] && cmp -s "$EXE" "$NEW/maker-connect-hub"; then
    say "the hub is $TAG already"
else
    CHANGED=1
    if [ -f "$EXE" ]; then
        # The hub dumps before its own updates (crate::backup); this replaces it from outside, so
        # a dump here first, the last 3 kept.
        install -d -m 0700 "$BACKUPS"
        DUMP=$BACKUPS/maker_connect-before-$TAG-$(date +%Y%m%d-%H%M%S).dump
        runuser -u postgres -- pg_dump -Fc maker_connect >"$DUMP"
        chmod 0600 "$DUMP"
        ls -1t "$BACKUPS"/maker_connect-before-*.dump | tail -n +4 | xargs -r rm -f
        say "database dumped to $DUMP"
        systemctl stop maker-connect-hub 2>/dev/null || true
    fi
    install -o maker-connect -g maker-connect -m 0755 "$NEW/maker-connect-hub" "$EXE"
    rm -rf "$HOME_DIR/spoolmandb"
    cp -r "$NEW/spoolmandb" "$HOME_DIR/spoolmandb"
    install -m 0644 "$NEW/LICENSE" "$NEW/SpoolmanDB-LICENSE" "$NEW/README.md" "$HOME_DIR/"
    chown -R maker-connect:maker-connect "$HOME_DIR/spoolmandb"
fi
cat >"$WORK/hub.service" <<EOF
# Written by scripts/hub-install.sh (Maker Connect): changes here are overwritten by its next run.
[Unit]
Description=Maker Connect hub
After=network-online.target postgresql@$PG-main.service
Wants=network-online.target
Requires=postgresql@$PG-main.service

[Service]
User=maker-connect
Group=maker-connect
WorkingDirectory=$HOME_DIR
EnvironmentFile=$ENV_FILE
Environment=PATH=/usr/lib/postgresql/$PG/bin:/usr/local/bin:/usr/bin:/bin
ExecStart=$EXE --listen 0.0.0.0:8460 --db maker-connect.db
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF
if ! cmp -s "$WORK/hub.service" "$UNIT"; then
    install -m 0644 "$WORK/hub.service" "$UNIT"
    systemctl daemon-reload
    CHANGED=1
fi
systemctl enable -q maker-connect-hub
# Restarted only when the build or its unit changed: a restart breaks every open connection.
if [ -n "$CHANGED" ]; then
    systemctl restart maker-connect-hub
else
    systemctl start maker-connect-hub
fi

IP=$(hostname -I 2>/dev/null | awk '{print $1}')
say "done: http://${IP:-<this machine>}:8460 ($TAG)"
for _ in $(seq 15); do
    CODE=$(journalctl -u maker-connect-hub -n 50 --no-pager -o cat 2>/dev/null | sed -n 's/.*setup code \([^ ]*\).*/\1/p' | tail -1)
    [ -n "$CODE" ] && break
    systemctl is-active -q maker-connect-hub || break
    sleep 1
done
if [ -n "${CODE:-}" ]; then
    say "no accounts yet: open the page and make the first with setup code $CODE"
fi
