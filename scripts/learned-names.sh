#!/usr/bin/env bash
# The names printers gave themselves in M115, as a hub kept them (#148; wiki Machine Profiles,
# "Learned from real printers"), added to profiles/learned.toml: each new name with its profile, its
# firmware and the day it was seen. A name already there for that profile is left as it is.
#
#   scripts/learned-names.sh [ssh-host]     the hub's machine (default root@192.168.60.7), whose
#                                           database the hub's install made (PostgreSQL, `maker_connect`)
#
# Then check the diff, run the hub's tests (profiles::tests), commit.
set -euo pipefail
cd "$(dirname "$0")/.."
host="${1:-root@192.168.60.7}"
rows=$(ssh "$host" "runuser -u postgres -- psql maker_connect -AtF '	' -c \
  \"SELECT machine_type, profile, coalesce(firmware, ''), to_char(seen_at, 'YYYY-MM-DD') FROM learned_names ORDER BY seen_at, machine_type\"" 2>/dev/null)
printf '%s\n' "$rows" | python3 -I -c '
import sys, tomllib, json
path = "profiles/learned.toml"
text = open(path, encoding="utf-8").read()
have = {(n["machine_type"].lower(), n["profile"]) for n in tomllib.loads(text).get("name", [])}
added = []
for line in sys.stdin.read().splitlines():
    if not line.strip():
        continue
    name, profile, firmware, seen = line.split("\t")
    if (name.lower(), profile) in have:
        continue
    have.add((name.lower(), profile))
    entry = f"\n[[name]]\nmachine_type = {json.dumps(name)}\nprofile = {json.dumps(profile)}\n"
    if firmware:
        entry += f"firmware = {json.dumps(firmware)}\n"
    entry += f"seen = {json.dumps(seen)}\n"
    added.append(entry)
    print(f"added: {name!r} -> {profile}")
if added:
    open(path, "a", encoding="utf-8").write("".join(added))
print(f"{len(added)} new name(s) in {path}")
'
