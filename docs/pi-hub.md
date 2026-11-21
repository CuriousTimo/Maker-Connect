# The hub on a Raspberry Pi (armv7, Debian 11)

How the hub runs on the test Pi (an Orange Pi PC, armv7, 1 GB): the database in TimescaleDB's container,
the hub native as a system service under a user of its own, `maker-connect`, which is not in the
`docker` group (the hub gets no Docker rights: anyone with the Docker socket is root on the Pi, and the
hub faces the network). Design: [plans/hub-core-v2.md](archive/plans/hub-core-v2.md) §2.7 "Pi deployment";
the database's roles: the design's database.md, "Roles and pools".

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
```

`db.env` (mode 0600) holds `POSTGRES_PASSWORD=…`.

Then, once, as the database's superuser: the hub's roles and the restore template
([scripts/db-roles.sql](../scripts/db-roles.sql); the hub logs in as `maker_connect_hub`, which owns
nothing and may only `SET ROLE`; the tables belong to `maker_connect_owner`, which has no password here: TimescaleDB needs it to have
LOGIN for a hypertable's background jobs, but without a password nobody logs in as it over the network),
and the database, owned by `maker_connect_owner`, with TimescaleDB in it:

```sh
docker exec -i maker-connect-db psql -U postgres -v ON_ERROR_STOP=1 -v hub_password='…' <scripts/db-roles.sql
docker exec -i maker-connect-db psql -U postgres -c "CREATE DATABASE maker_connect OWNER maker_connect_owner"
docker exec -i maker-connect-db psql -U postgres -d maker_connect -c "CREATE EXTENSION IF NOT EXISTS timescaledb"
```

The hub refuses to start when its login is a superuser, has `BYPASSRLS`, or owns or may read a table
itself.

## The PostgreSQL client

The hub dumps and restores with `pg_dump`, `pg_restore` and `psql` of the server's version (17.11 in
this image). Debian 11 on armhf has only version 13, and PostgreSQL's apt repository has no armhf
builds, so the three are built from PostgreSQL's source into the hub's folder `/var/lib/maker-connect/`
(its user first: [The hub](#the-hub); gcc, make, bison, flex and libssl-dev are on the Pi):

```sh
mkdir -p ~/pg17-build && cd ~/pg17-build
curl -fsSLO https://ftp.postgresql.org/pub/source/v17.11/postgresql-17.11.tar.bz2
curl -fsSLO https://ftp.postgresql.org/pub/source/v17.11/postgresql-17.11.tar.bz2.sha256
sha256sum -c postgresql-17.11.tar.bz2.sha256 && tar xjf postgresql-17.11.tar.bz2 && cd postgresql-17.11
./configure --prefix=/var/lib/maker-connect/pg17 --without-icu --without-readline \
  --without-zlib --without-zstd --without-lz4
for d in src/interfaces/libpq src/bin/pg_dump src/bin/psql; do make -j3 -C $d; done
for d in src/include src/interfaces/libpq src/bin/pg_dump src/bin/psql; do sudo make -C $d install; done
sudo chown -R maker-connect:maker-connect /var/lib/maker-connect/pg17
```

The hub is not given Docker's rights (`--pg-command "docker exec …"` would work, but anyone with the
Docker socket is root on the Pi, and the hub faces the network).

## The hub

The hub's user and folder `/var/lib/maker-connect/`: the binary, `pg17/` (the client above), and the
folders the hub makes beside `--db maker-connect.db`: `maker-connect-files/` and
`-dumps/` (0700; dumps 0600, the last 7 of each kind kept). The address of the database is in
`/etc/maker-connect/hub.env` (mode 0600, root's: systemd reads it), never on a command line:
`DATABASE_URL=postgres://maker_connect_hub:…@127.0.0.1:5432/maker_connect`.

```sh
sudo useradd --system --home /var/lib/maker-connect --create-home --shell /usr/sbin/nologin maker-connect
sudo install -o maker-connect -g maker-connect -m 0755 maker-connect-hub /var/lib/maker-connect/
sudo install -d -m 0755 /etc/maker-connect
sudo install -m 0600 /dev/null /etc/maker-connect/hub.env   # then write DATABASE_URL=… into it
id maker-connect                                             # no docker among its groups
```

`/etc/systemd/system/maker-connect-hub.service`:

```ini
[Unit]
Description=Maker Connect hub (v2, Postgres in container maker-connect-db)
After=network-online.target docker.service

[Service]
User=maker-connect
Group=maker-connect
WorkingDirectory=/var/lib/maker-connect
EnvironmentFile=/etc/maker-connect/hub.env
Environment=PATH=/var/lib/maker-connect/pg17/bin:/usr/local/bin:/usr/bin:/bin
ExecStart=/var/lib/maker-connect/maker-connect-hub --listen 0.0.0.0:8460 --db maker-connect.db
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```sh
sudo systemctl daemon-reload && sudo systemctl enable --now maker-connect-hub
```

On a fresh database the hub logs a setup code (`journalctl -u maker-connect-hub`); open
`http://<pi>:8460/setup` and make the first account with it.
