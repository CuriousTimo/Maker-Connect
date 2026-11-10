#!/usr/bin/env python3
"""Checks docs/buddy-audit.md against Prusa's Buddy firmware, mechanically, both ways.

Our hub plays Prusa Connect for printers on the Buddy firmware (MINI, MK4, XL, …). The Connect
server is closed; what binds it is what the firmware's open Connect client accepts and sends
(Prusa-Firmware-Buddy `src/connect`, its HTTP response parser, its dialog answers). Every item
below is taken from that source, never typed by hand, from both trees in reference/: 6.3.0
(what the MINI here runs) and master. Each must be named in the audit (in backticks).

The other way: every command name, argument, field, endpoint and header our hub uses (in its
code, tests left out) is one the firmware knows, or is named in the audit's section "What we
send or read that the firmware does not" with why.

A row may not be done "another way" (other route, in effect, differs, on purpose).

Run: python3 -I scripts/buddy-audit-check.py
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TREES = {"6.3.0": ROOT / "reference" / "buddy-6.3.0", "master": ROOT / "reference" / "Prusa-Firmware-Buddy"}
AUDIT_PATH = ROOT / "docs" / "buddy-audit.md"
AUDIT = AUDIT_PATH.read_text() if AUDIT_PATH.exists() else ""


def strip_comments(text):
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    text = re.sub(r"//[^\n]*", "", text)
    # A macro's own definition is no item (`#define R(NAME)`).
    return re.sub(r"^\s*#define [^\n]*", "", text, flags=re.M)


def sources(tree, pattern):
    return [(p, strip_comments(p.read_text(errors="replace"))) for p in sorted(tree.glob(pattern))]


items = {}  # kind -> {name: set of versions}


def add(kind, name, version):
    items.setdefault(kind, {}).setdefault(name, set()).add(version)


for version, tree in TREES.items():
    connect = sources(tree, "src/connect/*.[ch]pp")
    for path, text in connect:
        # Commands it takes from Connect: the table T("NAME", Type, args).
        if path.name == "command.cpp":
            for name in re.findall(r'\bT\("([A-Z_]+)"', text):
                add("command", name, version)
            # Arguments it reads from a command's kwargs.
            # An argument put together in a macro: "tools." #num ".high_flow" is tools.<n>.high_flow.
            for head, tail in re.findall(r'is_arg\("([^"]*)" #num "([^"]*)"', text):
                add("argument", f"{head}<n>{tail}", version)
            for name in re.findall(r'is_arg\("([^"]+)"(?! #num)', text):
                add("argument", name, version)
            for name in re.findall(r'\.key == "([^"]+)"', text):
                add("argument", name, version)
        # Fields it writes into what it sends (telemetry, events, info): JSON_FIELD_*("key".
        # The key is the call's first string (a condition may come before it: _G forms).
        for name in re.findall(r'JSON_[A-Z_]+\((?:[^"()\n]|\([^()\n]*\))*?"([A-Za-z_0-9.]+)"', text):
            add("field", name, version)
        # Keys written by hand into its JSON ("\"preview\":\"").
        for name in re.findall(r'\\"([a-z_][a-z0-9_]*)\\":', text):
            add("field", name, version)
        # The G-code metadata it passes on or leaves out ({ "key", MetaFilter::… }).
        for name, kind in re.findall(r'\{\s*"([a-z_A-Z0-9 \[\]]+)",\s*MetaFilter::(\w+)\s*\}', text):
            add("metadata", f"{name} ({kind})", version)
        # Values it writes or compares: states, event names, types (UPPER_CASE literals).
        if path.name in ("render.cpp", "planner.cpp", "printer.cpp", "marlin_printer.cpp", "connect.cpp"):
            for name in re.findall(r'"([A-Z][A-Z0-9_]{2,})"', text):
                add("value", name, version)
        # Endpoints it calls; the download put together from its prefix, IV and suffix.
        for name in re.findall(r'"(/(?:p|f|c)/[A-Za-z0-9_/{}%]*)', text):
            if name == "/f/" and 'enc_suffix = "/raw"' in text:
                name = "/f/<iv>/raw"
            add("endpoint", name, version)
        # Request headers it sends: { "Name", value, … }.
        for name in re.findall(r'\{\s*"([A-Z][A-Za-z]*(?:-[A-Za-z]+)*)",', text):
            add("request_header", name, version)
    # Response headers and values its HTTP parser knows (the automaton's names).
    for path, text in sources(tree, "src/common/http/resp_parser.cpp"):
        for name in re.findall(r"case Names::(\w+):", text):
            add("response", name, version)
    # The answers a dialog takes (DIALOG_ACTION's `button`).
    for path, text in sources(tree, "src/common/general_response.cpp"):
        for name in re.findall(r"\bR\((\w+)\)", text):
            add("dialog_answer", name, version)

# `--skeleton`: the audit's tables written from what is extracted (every row UNCHECKED), with
# whether our hub uses the item (found in its code), to be filled in by hand afterwards.
if "--skeleton" in sys.argv:
    print("run the checker without --skeleton after writing", file=sys.stderr)


# Every item named in the audit.
named = set(re.findall(r"`([^`]+)`", AUDIT))
missing = {k: sorted(n for n in v if n not in named) for k, v in items.items()}
missing = {k: v for k, v in missing.items() if v}

# No row done another way.
loose = []
for row in AUDIT.splitlines():
    if row.startswith("|") and row.count("|") > 3:
        status = row.rstrip("|").split("|")[-1]
        if re.search(r"other route|another route|in effect|differs|on purpose", status):
            loose.append(row.split("|")[1].strip())

# Tests a row cites must exist.
code = "\n".join(p.read_text() for p in (ROOT / "crates").rglob("*.rs"))
cited = set()
for row in AUDIT.splitlines():
    if row.startswith("|"):
        status = row.rstrip("|").split("|")[-1]
        if "test" in status:
            cited |= set(re.findall(r"`([a-z][a-z0-9_]+)`", status))
no_test = sorted(n for n in cited if not re.search(rf"\bfn {n}\(", code))

# The other way: what our hub uses, from its Connect code (tests left out).
HUB_FILES = ["prusa_connect.rs", "printer_files.rs", "main.rs", "farm.rs", "store.rs", "queue.rs"]


def hub_code(name):
    text = (ROOT / "crates" / "hub" / "src" / name).read_text()
    m = re.search(r"#\[cfg\(test\)\]\s*mod tests", text)
    return text[: m.start()] if m else text


hub = {name: hub_code(name) for name in HUB_FILES}
known = {k: set(v) for k, v in items.items()}
ours = {}  # kind -> {name: set of places}


def use(kind, name, where):
    ours.setdefault(kind, {}).setdefault(name, set()).add(where)


for name, text in hub.items():
    # Command names: "command": "NAME".
    for m in re.finditer(r'"command"\s*:\s*"([A-Z_]+)"', text):
        use("command", m.group(1), name)
    # Commands passed by name to a helper that sends {"command": name}.
    for m in re.finditer(r'"((?:SEND|START|STOP|PAUSE|RESUME|SET|CANCEL|DELETE|CREATE|RESET|DIALOG)_[A-Z_]+)"', text):
        use("command", m.group(1), name)
    # Arguments: keys inside "kwargs": { … }.
    for m in re.finditer(r'"kwargs"\s*:\s*\{([^{}]*)\}', text):
        for key in re.findall(r'"([a-z_][a-z0-9_.]*)"\s*:', m.group(1)):
            use("argument", key, name)
for name in ("prusa_connect.rs", "printer_files.rs"):
    text = hub[name]
    # Fields read from what the printer sends: .get("key"), ["key"], .pointer("/a/b").
    reads = re.findall(r'\.get\("([a-z_][a-z0-9_]*)"\)', text) + re.findall(r'\["([a-z_][a-z0-9_]*)"\](?!\s*=[^=])', text)
    for key in reads:
        use("field", key, name)
    for path in re.findall(r'\.pointer\("(/[^"]+)"\)', text):
        for key in path.strip("/").split("/"):
            if not key.isdigit():
                use("field", key, name)
    # Endpoints served.
    for path in re.findall(r'"(/(?:p|f|c)/[A-Za-z0-9_/{}%]*)"', text):
        use("endpoint", re.sub(r"\{[a-z_]+\}", lambda m: "<" + m.group(0)[1:-1] + ">", path), name)
    # Response headers sent: header names as literals.
    for header in re.findall(r'"((?:Command-Id|Code|Token|Content-Encryption-Mode|Content-Type|Content-Length|Connection|Sec-WebSocket-[A-Za-z]+|Upgrade)[^"]*)"', text):
        use("response_header", header, name)


if "--skeleton" in sys.argv:
    titles = {
        "command": "Commands it takes from Connect (`command.cpp`)",
        "argument": "Arguments it reads from a command (`command.cpp`)",
        "endpoint": "Endpoints it calls",
        "request_header": "Headers it sends",
        "response": "What it reads in an answer (`common/http/resp_parser.cpp`)",
        "field": "Fields it sends (telemetry, events, info)",
        "value": "Values it sends (states, events, types)",
        "metadata": "G-code metadata it passes on or leaves out (`render.cpp`)",
        "dialog_answer": "Answers a dialog takes (`common/general_response.cpp`)",
    }
    used_any = {}
    for kind, used in ours.items():
        for n, places in used.items():
            used_any.setdefault(n, set()).update(places)
    out = ["# Buddy firmware audit (Prusa Connect, the hub's side)", "",
           "Generated by `python3 -I scripts/buddy-audit-check.py --skeleton`; every item extracted",
           "from the firmware's source (6.3.0 and master). Status UNCHECKED until each row is checked.",
           "\"Hub uses\" says where our hub's code names it (found mechanically, not checked).", ""]
    for kind, title in titles.items():
        out += [f"## {title}", "", "| Item | In | Hub uses | Status |", "|---|---|---|---|"]
        for n in sorted(items.get(kind, {})):
            vs = items[kind][n]
            where = "both" if len(vs) == 2 else f"{next(iter(vs))} only"
            uses = ", ".join(sorted(used_any.get(n.split(" (")[0], set()))) or "—"
            out.append(f"| `{n}` | {where} | {uses} | UNCHECKED |")
        out.append("")
    out += ["## What we send or read that the firmware does not", "", "| Item | Where | Why | Status |", "|---|---|---|---|", ""]
    AUDIT_PATH.write_text("\n".join(out))
    print(f"wrote {AUDIT_PATH}")
    sys.exit(0)


def norm(s):
    return re.sub(r"[^a-z0-9]", "", s.lower())


section = AUDIT.split("## What we send or read that the firmware does not")[1] if "## What we send or read that the firmware does not" in AUDIT else ""
explained = set(re.findall(r"`([^`]+)`", section))
response_names = {norm(n) for n in known.get("response", set())}
extra = []
for kind, used in ours.items():
    for name, places in used.items():
        if kind == "response_header":
            ok = any(r.startswith(norm(name)) or norm(name).startswith(r) for r in response_names)
        elif kind == "field":
            ok = name in known.get("field", set()) or name in known.get("argument", set())
        else:
            ok = name in known.get(kind, set())
        if not ok and name not in explained:
            extra.append(f"{kind} {name}   ({', '.join(sorted(places))})")

total = sum(len(v) for v in items.values())
print(f"Buddy items: {total} ({', '.join(f'{k} {len(v)}' for k, v in items.items())})")
only = {k: sorted(n for n, vs in v.items() if vs == {'master'}) for k, v in items.items()}
only_old = {k: sorted(n for n, vs in v.items() if vs == {'6.3.0'}) for k, v in items.items()}
print(f"  only in master: {sum(len(v) for v in only.values())}; only in 6.3.0: {sum(len(v) for v in only_old.values())}")
print(f"what our hub uses: {sum(len(v) for v in ours.values())} ({', '.join(f'{k} {len(v)}' for k, v in ours.items())})")
failed = False
if missing:
    failed = True
    print(f"NOT in the audit: {sum(len(v) for v in missing.values())}")
    for kind, names in missing.items():
        print(f"  {kind}: {', '.join(names)}")
if loose:
    failed = True
    print(f"rows done another way (not allowed): {', '.join(loose)}")
if no_test:
    failed = True
    print(f"tests named in the audit that do not exist: {', '.join(no_test)}")
if extra:
    failed = True
    print(f"used by our hub, not the firmware's, not explained: {len(extra)}")
    for e in sorted(extra):
        print(f"  {e}")
if failed:
    sys.exit(1)
print("every item is in the audit; everything our hub uses is the firmware's or explained")
