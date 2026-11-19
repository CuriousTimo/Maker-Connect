# The hub on a Raspberry Pi (armv7, Debian 11)

How the hub runs on the test Pi (an Orange Pi PC, armv7, 1 GB): the database in TimescaleDB's container,
the hub native as a `systemd --user` service. Design: [plans/hub-core-v2.md](plans/hub-core-v2.md) §2.7
"Pi deployment".

## The database

Docker with AppArmor (`sudo apt install apparmor && sudo systemctl restart docker`: without
`apparmor_parser` Docker cannot load its `docker-default` profile and starts no container). The
container runs confined by `docker-default`, listens on `127.0.0.1` only, keeps its data on a named
volume and starts again at boot:

```sh
docker volume create maker-connect-db-data
docker run -d --name maker-connect-db --restart unless-stopped \
  --memory 384m --env-file ~/maker-connect-hub/db.env \
  -p 127.0.0.1:5432:5432 -v maker-connect-db-data:/home/postgres/pgdata \
  timescale/timescaledb:2.32.0-pg17 \
  postgres -c shared_buffers=64MB -c work_mem=4MB -c max_connections=20
docker exec -i maker-connect-db psql -U postgres -c "CREATE DATABASE maker_connect"
```

`db.env` (mode 0600) holds `POSTGRES_PASSWORD=…`.

## The PostgreSQL client

The hub dumps and restores with `pg_dump`, `pg_restore` and `psql` of the server's version (17.11 in
this image). Debian 11 on armhf has only version 13, and PostgreSQL's apt repository has no armhf
builds, so the three are built from PostgreSQL's source into the hub's folder (no root needed; gcc,
make, bison, flex and libssl-dev are on the Pi):

```sh
mkdir -p ~/pg17-build && cd ~/pg17-build
curl -fsSLO https://ftp.postgresql.org/pub/source/v17.11/postgresql-17.11.tar.bz2
curl -fsSLO https://ftp.postgresql.org/pub/source/v17.11/postgresql-17.11.tar.bz2.sha256
sha256sum -c postgresql-17.11.tar.bz2.sha256 && tar xjf postgresql-17.11.tar.bz2 && cd postgresql-17.11
./configure --prefix=$HOME/maker-connect-hub/pg17 --without-icu --without-readline \
  --without-zlib --without-zstd --without-lz4
for d in src/interfaces/libpq src/bin/pg_dump src/bin/psql; do make -j3 -C $d; done
for d in src/include src/interfaces/libpq src/bin/pg_dump src/bin/psql; do make -C $d install; done
```

The hub is not given Docker's rights (`--pg-command "docker exec …"` would work, but anyone with the
Docker socket is root on the Pi, and the hub faces the network).

## The hub

In `~/maker-connect-hub/`: the binary, `hub.env` (mode 0600,
`DATABASE_URL=postgres://postgres:…@127.0.0.1:5432/maker_connect`), and the folders the hub makes
beside `--db maker-connect.db`: `maker-connect-files/`, `-downloads/` and `-dumps/` (0700; dumps 0600,
the last 7 of each kind kept). `~/.config/systemd/user/maker-connect-hub.service`:

```ini
[Unit]
Description=Maker Connect hub (v2, Postgres in container maker-connect-db)
After=network-online.target

[Service]
WorkingDirectory=%h/maker-connect-hub
EnvironmentFile=%h/maker-connect-hub/hub.env
Environment=PATH=%h/maker-connect-hub/pg17/bin:/usr/local/bin:/usr/bin:/bin
ExecStart=%h/maker-connect-hub/maker-connect-hub --listen 0.0.0.0:8460 --db maker-connect.db
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

```sh
loginctl enable-linger pi          # user services start at boot
systemctl --user daemon-reload && systemctl --user enable --now maker-connect-hub
```

On a fresh database the hub logs a setup code (`journalctl --user -u maker-connect-hub`); open
`http://<pi>:8460/setup` and make the first account with it.
