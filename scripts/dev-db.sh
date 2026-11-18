#!/usr/bin/env bash
# The hub's development database, on demand (never a background service on the Mac): Postgres with
# TimescaleDB in a container, the image the plan names (docs/plans/hub-core-v2.md §2.7).
#
#   scripts/dev-db.sh up        start Colima (if it is not running) and the container; prints DATABASE_URL
#   scripts/dev-db.sh down      remove the container; stop Colima if `up` started it
#   scripts/dev-db.sh prepare   refresh the queries' offline cache (crates/hub/.sqlx/) against a fresh
#                               database with every migration applied (needs `up` first and sqlx-cli)
#   scripts/dev-db.sh url       print DATABASE_URL
#
# `up` leaves a mark in the state directory when it started Colima, so `down` stops only what `up` started.
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"

IMAGE="timescale/timescaledb:2.32.0-pg17"
NAME="mc-dev-db"
PORT=55432
PASSWORD="mc-dev"
URL="postgres://postgres:${PASSWORD}@127.0.0.1:${PORT}/postgres"
STATE="${TMPDIR:-/tmp}/maker-connect-dev-db"
mkdir -p "$STATE"

colima_running() { colima status >/dev/null 2>&1; }

up() {
    if ! colima_running; then
        echo "dev-db: starting Colima" >&2
        colima start >&2
        touch "$STATE/colima-started"
    fi
    if [ -z "$(docker ps -q --filter "name=^${NAME}$")" ]; then
        docker rm -f "$NAME" >/dev/null 2>&1 || true
        docker run -d --name "$NAME" -p "127.0.0.1:${PORT}:5432" \
            -e POSTGRES_PASSWORD="$PASSWORD" "$IMAGE" >/dev/null
        touch "$STATE/container-started"
    fi
    for _ in $(seq 1 60); do
        if docker exec "$NAME" pg_isready -U postgres -h 127.0.0.1 >/dev/null 2>&1; then
            echo "DATABASE_URL=$URL"
            return 0
        fi
        sleep 1
    done
    echo "dev-db: the database did not come up" >&2
    return 1
}

down() {
    docker rm -f "$NAME" >/dev/null 2>&1 || true
    rm -f "$STATE/container-started"
    if [ -f "$STATE/colima-started" ]; then
        echo "dev-db: stopping Colima" >&2
        colima stop >&2
        rm -f "$STATE/colima-started"
    fi
}

# Every module's migrations, in version order across modules (the order the hub's migrator runs them).
migrations() {
    find crates/hub/src -path '*/migrations/*.sql' | awk -F/ '{print $NF "\t" $0}' | sort | cut -f2
}

prepare() {
    docker exec -e PGOPTIONS=--client-min-messages=warning "$NAME" psql -q -U postgres -c "DROP DATABASE IF EXISTS mc_prepare" -c "CREATE DATABASE mc_prepare"
    for f in $(migrations); do
        docker exec -i -e PGOPTIONS=--client-min-messages=warning "$NAME" psql -q -v ON_ERROR_STOP=1 -U postgres -d mc_prepare <"$f" >/dev/null
    done
    (cd crates/hub && SQLX_OFFLINE=false DATABASE_URL="postgres://postgres:${PASSWORD}@127.0.0.1:${PORT}/mc_prepare" \
        cargo sqlx prepare "$@" -- --all-targets --features pg-tests)
}

case "${1:-}" in
    up) up ;;
    down) down ;;
    prepare) shift; prepare "$@" ;;
    url) echo "$URL" ;;
    *) echo "usage: $0 up|down|prepare|url" >&2; exit 2 ;;
esac
