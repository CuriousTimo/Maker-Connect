#!/usr/bin/env python3
"""Summarizes a fake_mk3 log: per command (numbering and checksum taken off) how often it came,
when first, and the gaps between its sendings."""
import re, sys
from collections import defaultdict

seen = defaultdict(list)
order = []
for line in open(sys.argv[1]):
    m = re.match(r"\s*([\d.]+) RX (.*)", line)
    if not m:
        continue
    t, cmd = float(m.group(1)), m.group(2)
    cmd = re.sub(r"^N\d+ ", "", cmd)
    cmd = re.sub(r"\*\d+$", "", cmd).strip()
    if cmd not in seen:
        order.append(cmd)
    seen[cmd].append(t)
for cmd in order:
    ts = seen[cmd]
    gaps = [round(b - a, 1) for a, b in zip(ts, ts[1:])]
    g = sorted(set(gaps))
    print(f"{len(ts):4}x  first {ts[0]:7.2f}s  gaps {g[:6]}{'…' if len(g) > 6 else ''}  {cmd}")
