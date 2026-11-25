#!/usr/bin/env bash
# The Maker Connect hub on Proxmox VE, in one line in the Proxmox shell (as root):
#
#   bash -c "$(curl -fsSL https://raw.githubusercontent.com/CuriousTimo/Maker-Connect/main/scripts/proxmox-hub.sh)"
#
# On the Proxmox host it asks a few questions (each with a default), makes an unprivileged Debian 13
# container, and installs the hub in it with scripts/hub-install.sh. It writes /usr/bin/update into
# the container: `update` there runs this same line, which, inside a container, updates the hub
# (hub-install.sh again: Debian's packages, and the hub to the newest signed release).
#
# The way community-scripts.org's container scripts work, without their framework: nothing else is
# fetched at run time, and nothing is reported anywhere.
set -euo pipefail

RAW=https://raw.githubusercontent.com/CuriousTimo/Maker-Connect/main/scripts
LINE="bash -c \"\$(curl -fsSL $RAW/proxmox-hub.sh)\""

say() { printf '\e[1;32m==\e[0m %s\n' "$*"; }
fail() { printf '\e[1;31mproxmox-hub:\e[0m %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" = 0 ] || fail "run it as root"

# --- Inside the container (no Proxmox here): update ------------------------------------------
if ! command -v pveversion >/dev/null 2>&1; then
    say "updating the Maker Connect hub"
    exec bash -c "$(curl -fsSL "$RAW/hub-install.sh")"
fi

# --- On the Proxmox host: the questions ------------------------------------------------------
command -v whiptail >/dev/null || fail "whiptail is missing (apt install whiptail)"
TITLE="Maker Connect hub"
ask() { # ask <question> <default>
    whiptail --title "$TITLE" --inputbox "$1" 9 64 "$2" 3>&1 1>&2 2>&3 || fail "stopped"
}
pick() { # pick <question> <tag> <text> ...
    local q=$1
    shift
    whiptail --title "$TITLE" --menu "$q" 16 64 6 "$@" 3>&1 1>&2 2>&3 || fail "stopped"
}
storages() { # storages <content>: "<name> <type>" for each storage that holds it
    pvesm status -content "$1" 2>/dev/null | awk 'NR > 1 {print $1, $2}'
}

whiptail --title "$TITLE" --yesno "This makes a Debian 13 container (LXC) on this Proxmox host and installs the Maker Connect hub in it, with PostgreSQL 17 and TimescaleDB.\n\nGo on?" 12 64 || fail "stopped"

CTID=$(ask "Container ID" "$(pvesh get /cluster/nextid)")
pct status "$CTID" >/dev/null 2>&1 && fail "container $CTID exists already"
HOSTNAME=$(ask "Host name" "maker-connect-hub")
CORES=$(ask "CPU cores" "2")
MEMORY=$(ask "Memory (MiB)" "2048")
DISK=$(ask "Disk (GB)" "16")
BRIDGE=$(ask "Network bridge" "vmbr0")
if [ "$(pick "Address" dhcp "DHCP" static "Static address")" = static ]; then
    ADDRESS=$(ask "Address with prefix (such as 192.168.1.50/24)" "")
    GATEWAY=$(ask "Gateway" "")
    DNS=$(ask "DNS server(s), space-separated" "$(awk '$1 == "nameserver" {printf "%s%s", s, $2; s=" "}' /etc/resolv.conf)")
    NET="name=eth0,bridge=$BRIDGE,ip=$ADDRESS,gw=$GATEWAY"
    DNS_OPTS=(--nameserver "$DNS")
else
    NET="name=eth0,bridge=$BRIDGE,ip=dhcp"
    DNS_OPTS=()
fi
while :; do
    PASSWORD=$(whiptail --title "$TITLE" --passwordbox "Root password for the container\n(empty: none, and its console logs in as root by itself)" 10 64 3>&1 1>&2 2>&3) || fail "stopped"
    [ -z "$PASSWORD" ] && break
    AGAIN=$(whiptail --title "$TITLE" --passwordbox "The root password again" 9 64 3>&1 1>&2 2>&3) || fail "stopped"
    [ "$PASSWORD" = "$AGAIN" ] && break
    whiptail --title "$TITLE" --msgbox "The two passwords differ: try again." 8 64
done
# shellcheck disable=SC2046
STORAGE=$(pick "Storage for the container's disk" $(storages rootdir))
# shellcheck disable=SC2046
TEMPLATES=$(pick "Storage for the Debian template" $(storages vztmpl))

# --- The Debian 13 template ------------------------------------------------------------------
say "the Debian 13 template"
pveam update >/dev/null
TEMPLATE=$(pveam available --section system | awk '{print $2}' | grep "^debian-13-standard_.*_$(dpkg --print-architecture)\.tar" | sort -V | tail -1)
[ -n "$TEMPLATE" ] || fail "no Debian 13 template on Proxmox's list (pveam available)"
if ! pveam list "$TEMPLATES" | awk '{print $1}' | grep -q "/$TEMPLATE\$"; then
    pveam download "$TEMPLATES" "$TEMPLATE"
fi

# --- The container ---------------------------------------------------------------------------
say "container $CTID ($HOSTNAME)"
pct create "$CTID" "$TEMPLATES:vztmpl/$TEMPLATE" \
    --hostname "$HOSTNAME" --ostype debian --unprivileged 1 --features nesting=1 \
    --cores "$CORES" --memory "$MEMORY" \
    --rootfs "$STORAGE:$DISK" --net0 "$NET" "${DNS_OPTS[@]}" --onboot 1 \
    --description "Maker Connect hub: update with \`update\` in its console."
pct start "$CTID"
for _ in $(seq 60); do
    pct exec "$CTID" -- getent hosts github.com >/dev/null 2>&1 && break
    sleep 1
done
pct exec "$CTID" -- getent hosts github.com >/dev/null 2>&1 || fail "container $CTID has no network"
# The password over stdin, never on a command line (anyone on the host could read it there).
if [ -n "$PASSWORD" ]; then
    printf 'root:%s\n' "$PASSWORD" | pct exec "$CTID" -- chpasswd
else
    pct exec "$CTID" -- bash -c 'mkdir -p /etc/systemd/system/container-getty@1.service.d && cat >/etc/systemd/system/container-getty@1.service.d/override.conf <<"EOF"
[Service]
ExecStart=
ExecStart=-/sbin/agetty --autologin root --noclear --keep-baud tty%I 115200,38400,9600 $TERM
EOF
systemctl daemon-reload && systemctl restart container-getty@1'
fi
unset PASSWORD AGAIN

# --- The hub inside --------------------------------------------------------------------------
say "installing the hub in container $CTID"
pct exec "$CTID" -- bash -c "apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq curl ca-certificates >/dev/null"
pct exec "$CTID" -- bash -c "$(curl -fsSL "$RAW/hub-install.sh")"
pct exec "$CTID" -- bash -c "printf '%s\n' '#!/bin/sh' '$LINE' >/usr/bin/update && chmod 0755 /usr/bin/update"

IP=$(pct exec "$CTID" -- hostname -I | awk '{print $1}')
say "done: the hub is at http://${IP:-<container $CTID>}:8460"
say "to update it later: open container $CTID's console and type: update"
