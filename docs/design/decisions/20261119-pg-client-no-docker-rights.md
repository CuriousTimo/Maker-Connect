# PostgreSQL client from source; the hub gets no Docker rights

Date: 2026-11-19  
Status: accepted  
Decided by: owner (journal:V2-STORE-5:0014)

## Context

Debian 11 armhf has pg client 13, the server is 17; `docker exec` would work but anyone with the Docker socket is root, and the hub faces the network.

## Decision

`pg_dump`, `pg_restore`, `psql` of the server's version are built from PostgreSQL's source into the hub's folder and put on its `PATH`; the hub is never in the docker group. `--pg-command` stays for other hosts, empty on .67. Supersedes the `docker exec` choice of journal:V2-STORE-5:0013.

## Where it shows

[storage-ops.md](../storage-ops.md).
