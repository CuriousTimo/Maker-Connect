#!/usr/bin/env python3
"""Checks docs/audits/buddy.md against Prusa's Buddy firmware, mechanically, both ways.

Our hub plays Prusa Connect for printers on the Buddy firmware (MINI, MK4, XL, …). The Connect
server is closed; what binds it is what the firmware's open Connect client accepts and sends
(Prusa-Firmware-Buddy `src/connect`, its HTTP response parser, its dialog answers). Every item
below is taken from that source, never typed by hand, from one tree in reference/: Buddy 8.0.2
(`reference/buddy-8.0.2`, the current release; the only one supported, owner 2026-11-24, #3).
Each must be named in the audit (in backticks).

The other way: every command name, argument, field, endpoint and header our hub uses (in its
code, tests left out) is one the firmware knows, or is named in the audit's section "What we
send or read that the firmware does not" with why.

A row may not be done "another way" (other route, in effect, differs, on purpose).

Run: python3 -I scripts/buddy-audit-check.py
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# The references are gitignored: in a git worktree (.../.claude/worktrees/<name>) they are read
# from the main checkout.
REF = ROOT / "reference"
if not REF.exists() and ROOT.parent.name == "worktrees":
    REF = ROOT.parent.parent.parent / "reference"
TREE = REF / "buddy-8.0.2"
AUDIT_PATH = ROOT / "docs" / "audits" / "buddy.md"
AUDIT = AUDIT_PATH.read_text() if AUDIT_PATH.exists() else ""


def strip_comments(text):
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    text = re.sub(r"//[^\n]*", "", text)
    # A macro's own definition is no item (`#define R(NAME)`).
    return re.sub(r"^\s*#define [^\n]*", "", text, flags=re.M)


def sources(tree, pattern):
    return [(p, strip_comments(p.read_text(errors="replace"))) for p in sorted(tree.glob(pattern))]


items = {}  # kind -> set of names


def add(kind, name):
    items.setdefault(kind, set()).add(name)


for tree in (TREE,):
    connect = sources(tree, "src/connect/*.[ch]pp")
    for path, text in connect:
        # Commands it takes from Connect: the table T("NAME", Type, args).
        if path.name == "command.cpp":
            for name in re.findall(r'\bT\("([A-Z_]+)"', text):
                add("command", name)
            # Arguments it reads from a command's kwargs.
            # An argument put together in a macro: "tools." #num ".high_flow" is tools.<n>.high_flow.
            for head, tail in re.findall(r'is_arg\("([^"]*)" #num "([^"]*)"', text):
                add("argument", f"{head}<n>{tail}")
            for name in re.findall(r'is_arg\("([^"]+)"(?! #num)', text):
                add("argument", name)
            for name in re.findall(r'\.key == "([^"]+)"', text):
                add("argument", name)
        # Fields it writes into what it sends (telemetry, events, info): JSON_FIELD_*("key".
        # The key is the call's first string (a condition may come before it: _G forms).
        for name in re.findall(r'JSON_[A-Z_]+\((?:[^"()\n]|\([^()\n]*\))*?"([A-Za-z_0-9.]+)"', text):
            add("field", name)
        # Keys written by hand into its JSON ("\"preview\":\"").
        for name in re.findall(r'\\"([a-z_][a-z0-9_]*)\\":', text):
            add("field", name)
        # The G-code metadata it passes on or leaves out ({ "key", MetaFilter::… }).
        for name, kind in re.findall(r'\{\s*"([a-z_A-Z0-9 \[\]]+)",\s*MetaFilter::(\w+)\s*\}', text):
            add("metadata", f"{name} ({kind})")
        # Values it writes or compares: states, event names, types (UPPER_CASE literals).
        if path.name in ("render.cpp", "planner.cpp", "printer.cpp", "marlin_printer.cpp", "connect.cpp"):
            for name in re.findall(r'"([A-Z][A-Z0-9_]{2,})"', text):
                add("value", name)
        # Endpoints it calls; the download put together from its prefix, IV and suffix.
        for name in re.findall(r'"(/(?:p|f|c)/[A-Za-z0-9_/{}%]*)', text):
            if name == "/f/" and 'enc_suffix = "/raw"' in text:
                name = "/f/<iv>/raw"
            add("endpoint", name)
        # Request headers it sends: { "Name", value, … }.
        for name in re.findall(r'\{\s*"([A-Z][A-Za-z]*(?:-[A-Za-z]+)*)",', text):
            add("request_header", name)
    # Response headers and values its HTTP parser knows (the automaton's names).
    for path, text in sources(tree, "src/common/http/resp_parser.cpp"):
        for name in re.findall(r"case Names::(\w+):", text):
            add("response", name)
    # The states it writes (to_str(DeviceState)).
    for path, text in sources(tree, "src/state/printer_state.cpp"):
        block = text.split("const char *to_str(DeviceState state)")[1].split("\n}\n")[0]
        for name in re.findall(r'return "([A-Z]+)";', block):
            add("state", name)
    # The answers a dialog takes (DIALOG_ACTION's `button`).
    for path, text in sources(tree, "src/common/general_response.cpp"):
        for name in re.findall(r"\bR\((\w+)\)", text):
            add("dialog_answer", name)

# `--skeleton`: the audit's tables written from what is extracted (every row UNCHECKED), with
# whether our hub uses the item (found in its code), to be filled in by hand afterwards.


# Every item named in the audit.
named = set(re.findall(r"`([^`]+)`", AUDIT))
missing = {k: sorted(n for n in v if n not in named) for k, v in items.items()}
missing = {k: v for k, v in missing.items() if v}

# No row done another way.
loose = []
for row in AUDIT.splitlines():
    if row.startswith("|") and row.count("|") > 3:
        status = row.rstrip("|").split("|")[-1]
        if re.search(r"other route|another route|in effect|on purpose", status):
            loose.append(row.split("|")[1].strip())

code = "\n".join(p.read_text() for p in (ROOT / "crates").rglob("*.rs"))
no_test = []

# The other way: what our hub uses, from its Connect code (tests left out).
HUB_FILES = [
    "devices/prusa_connect.rs",
    "devices/printer_files.rs",
    # A Buddy printer's tools as Buddy 8.0.2 writes them; a device on the agent's as PrusaLink.
    "devices/connect_tools.rs",
    "devices/buddy_tools.rs",
    "devices/sdk_tools.rs",
    # Buddy's download over the WebSocket (#212).
    "devices/inline_download.rs",
    # What was main.rs: the binary, the library root, the agent link, the commands, the router and pages.
    "main.rs",
    "lib.rs",
    "link.rs",
    "commands.rs",
    "devices/legacy_gcode.rs",
    "web/router.rs",
    "web/shell.rs",
    "web/machines.rs",
    "farm.rs",
    # What was store.rs.
    "store/mod.rs",
    "store/files.rs",
    "store/pg_jobs.rs",
    "store/agents.rs",
    "store/history.rs",
    "store/commands.rs",
    "store/machines.rs",
    "queue.rs",
]


def hub_code(name):
    text = (ROOT / "crates" / "hub" / "src" / name).read_text()
    m = re.search(r"#\[cfg\(test\)\]\s*mod tests", text)
    return text[: m.start()] if m else text


hub = {name: hub_code(name) for name in HUB_FILES}
known = items
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
for name in ("devices/prusa_connect.rs", "devices/printer_files.rs", "devices/connect_tools.rs",
             "devices/buddy_tools.rs", "devices/sdk_tools.rs", "devices/inline_download.rs"):
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
           "from the firmware's source (Buddy 8.0.2). Status UNCHECKED until each row is checked.",
           "\"Hub uses\" says where our hub's code names it (found mechanically, not checked).", ""]
    for kind, title in titles.items():
        out += [f"## {title}", "", "| Item | Hub uses | Status |", "|---|---|---|"]
        for n in sorted(items.get(kind, set())):
            uses = ", ".join(sorted(used_any.get(n.split(" (")[0], set()))) or "—"
            out.append(f"| `{n}` | {uses} | UNCHECKED |")
        out.append("")
    out += ["## What we send or read that the firmware does not", "", "| Item | Where | Why | Status |", "|---|---|---|---|", ""]
    # Printed, never written over the audit: the audit holds hand-checked rows.
    print("\n".join(out))
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


# ---- the rows: Result, test, cited lines, the two ways of "not used" ---------------------------
# A row of the tables with a Result column: | Item | Buddy 8.0.2 | Hub | Result | Test |.
RESULTS = ("**identical**", "**differs** D", "**not used by the hub**", "**n/a**", "**OPEN**", "UNCHECKED")
SRC_DIRS = [TREE / "src" / d for d in ("connect", "common", "common/http", "state")]
cite_re = re.compile(r"^([A-Za-z0-9_./-]+\.(?:cpp|hpp|h|rs)):(\d+)(?:=(.+))?$")
file_lines = {}


def lines_of(path):
    if path not in file_lines:
        file_lines[path] = path.read_text(errors="replace").splitlines()
    return file_lines[path]


def resolve(name):
    if name.endswith(".rs"):
        return ROOT / "crates" / "hub" / "src" / name
    if "/" in name:
        return TREE / name
    for d in SRC_DIRS:
        if (d / name).exists():
            return d / name
    return None


def split_row(row):
    return [c.strip() for c in row.strip().strip("|").split("|")]


problems = []
tally = {}
section = ""
cols = None
checked_rows = {}  # (section, item) -> (result kind)
row_tests = {}  # (section, item) -> the row's Test cell
diffs_named = set(re.findall(r"^\| (D\d+) \|", AUDIT, re.M))
for row in AUDIT.splitlines():
    if row.startswith("## "):
        section = row[3:].strip()
        cols = None
        continue
    if not row.startswith("|"):
        continue
    cells = split_row(row)
    if cells[0] == "Item" and "Result" in cells:
        cols = cells
        continue
    if cols is None or set(cells[0]) <= set("-"):
        continue
    if len(cells) != len(cols):
        problems.append(f"{section}: row with {len(cells)} cells, header has {len(cols)}: {cells[0]}")
        continue
    cell = dict(zip(cols, cells))
    item = cell["Item"].strip("`")
    result = cell["Result"]
    kind = next((r for r in RESULTS if result.startswith(r)), None)
    if kind is None:
        problems.append(f"{section}: {item}: no Result ({result[:40]})")
        continue
    kind = kind.replace("**", "").replace(" D", "")
    tally.setdefault(section, {}).setdefault(kind, 0)
    tally[section][kind] += 1
    checked_rows[(section, item)] = kind
    row_tests[(section, item)] = cell.get("Test", "")
    # Cited lines: every `file:line` must hold the item (or the anchor after `=`).
    for col in ("Buddy 8.0.2", "Hub"):
        for tok in re.findall(r"`([^`]+)`", cell.get(col, "")):
            m = cite_re.match(tok)
            if not m:
                continue
            path = resolve(m.group(1))
            anchor = m.group(3) or item
            if path is None or not path.exists():
                problems.append(f"{section}: {item}: no file {m.group(1)}")
                continue
            lines = lines_of(path)
            n = int(m.group(2))
            if n < 1 or n > len(lines):
                problems.append(f"{section}: {item}: {tok}: no such line")
            elif col == "Hub" and anchor not in lines[n - 1]:
                problems.append(f"{section}: {item}: {tok}: the line does not hold {anchor!r}")
            elif col == "Buddy 8.0.2" and anchor not in lines[n - 1] and m.group(3) is not None:
                problems.append(f"{section}: {item}: {tok}: the line does not hold {anchor!r}")
            elif col == "Buddy 8.0.2" and m.group(3) is None:
                bare = re.sub(r"\s*\(.*\)$", "", item)
                if bare.split("<n>")[0] not in lines[n - 1] and not any(part in lines[n - 1] for part in bare.split("<n>")):
                    problems.append(f"{section}: {item}: {tok}: the line does not hold {bare!r}")
    # Tests named must exist; identical and differs need one.
    tests = re.findall(r"`([a-z][a-z0-9_]+)`", cell.get("Test", ""))
    for t in tests:
        if not re.search(rf"\bfn {t}\(", code):
            problems.append(f"{section}: {item}: test {t} does not exist")
    if kind in ("identical", "differs") and not tests:
        problems.append(f"{section}: {item}: {kind} without a test")
    if kind == "differs":
        d = re.match(r"\*\*differs\*\* (D\d+)", result)
        if not d or d.group(1) not in diffs_named:
            problems.append(f"{section}: {item}: names no difference in the Differences section")

# "Not used by the hub" and "used" against the hub's code (the two kinds the extraction of the hub covers).
for (sec, item), kind in checked_rows.items():
    for k, title in (("command", "Commands it takes"), ("argument", "Arguments it reads"), ("endpoint", "Endpoints it calls")):
        if sec.startswith(title):
            used = item in ours.get(k, {})
            # Named by the hub for a device on the agent only: allowed when the row names the test
            # that shows none of it goes to a Buddy printer.
            agent_only = "`agent_names_never_go_to_a_buddy`" in row_tests.get((sec, item), "")
            if kind == "not used by the hub" and used and not agent_only:
                problems.append(f"{sec}: {item}: says not used, but the hub names it: {sorted(ours[k][item])}")
            if kind in ("identical", "differs") and not used and k == "command":
                problems.append(f"{sec}: {item}: says used, but the hub's code does not name it")

# The test file's own copies of the firmware's lists equal the firmware's.
test_src = (ROOT / "crates" / "hub" / "src" / "devices" / "buddy_audit.rs").read_text()
table_block = test_src.split("const TABLE")[1].split("];")[0]
table_names = set(re.findall(r'\(\s*"([A-Z_]+)",\s*"[A-Za-z]+",', table_block))
if table_names != items.get("command", set()):
    problems.append("buddy_audit.rs TABLE differs from command.cpp's: " + str(sorted(table_names ^ set(items.get("command", {})))))
responses_block = test_src.split("pub const RESPONSES")[1].split("];")[0]
responses = re.findall(r'"([A-Za-z0-9_]+)"', responses_block)
firmware_responses = list(items.get("dialog_answer", set()))
if sorted(responses) != sorted(firmware_responses):
    problems.append("buddy_audit.rs RESPONSES differs from general_response.cpp's: " + str(sorted(set(responses) ^ set(firmware_responses))))
# The command parser port's state names.

total = sum(len(v) for v in items.values())
print(f"Buddy items: {total} ({', '.join(f'{k} {len(v)}' for k, v in items.items())})")
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
if problems:
    failed = True
    print(f"rows that do not hold: {len(problems)}")
    for p in problems:
        print(f"  {p}")
# The HTTP answer automaton the tests run (`crates/hub/src/devices/buddy_http.rs`) is the one
# Buddy's own generator makes (#202).
automaton = subprocess.run(
    [sys.executable, "-I", str(ROOT / "scripts" / "buddy-http-automaton.py"), "--check"],
    capture_output=True, text=True,
)
if automaton.returncode != 0:
    failed = True
    print(automaton.stdout.strip() or automaton.stderr.strip())
print("rows by result:")
for sec, t in tally.items():
    print(f"  {sec}: " + ", ".join(f"{k} {v}" for k, v in sorted(t.items())))
if failed:
    sys.exit(1)
print("every item is in the audit; every checked row holds; everything our hub uses is the firmware's or explained")
