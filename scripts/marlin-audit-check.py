#!/usr/bin/env python3
"""Checks docs/marlin-audit.md against OctoPrint's serial layer and Marlin, mechanically.

For every printer that is not Prusa's MK3 family, the reference is OctoPrint's serial
connector with its default settings (reference/OctoPrint/src/octoprint/plugins/serial_connector/
serial_comm.py and config_schema.py, the default gcode scripts in octoprint/schema/config/
scripts.py) and, for which G-codes exist, the Marlin source (reference/Marlin/Marlin/src/gcode/
gcode.cpp). Where they disagree, the firmware source decides.

Every item below is taken from that source by a regular expression, never typed by hand:
the regexes the printer's output is read with, the literal line checks, the error lists, the
M115 capabilities, the firmware-name rules, the action commands, the G-codes sent, the
handlers per G-code, the default settings (timeouts, intervals, lists), the methods that do
the talking. Each must be named in the audit (in backticks). Prints what is missing and
exits 1 if anything is.

Then the other way round: every G-code our agent can send (literals in crates/agent/src
and the hub's sd_card.rs, main.rs, queue.rs, test code left out) must be one OctoPrint
sends, in its form, or be named in the audit's section "What we send that OctoPrint does not".
Marlin's own list is the reference for "does the firmware have it at all".

Statuses in the audit: UNCHECKED, or a status that names tests (`fn name(` must exist and
contain the row's G-codes). "same"/"fixed" without a named test is not accepted, and neither
is "another way / in effect / differs / on purpose" (to the letter).

Run: python3 -I scripts/marlin-audit-check.py
"""
import ast
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OCTO = ROOT / "reference" / "OctoPrint" / "src" / "octoprint"
SERIAL = OCTO / "plugins" / "serial_connector"
COMM_PATH = SERIAL / "serial_comm.py"
COMM = COMM_PATH.read_text()
SCHEMA = (SERIAL / "config_schema.py").read_text()
SCRIPTS = (OCTO / "schema" / "config" / "scripts.py").read_text()
MARLIN = (ROOT / "reference" / "Marlin" / "Marlin" / "src" / "gcode" / "gcode.cpp").read_text()
AUDIT = (ROOT / "docs" / "marlin-audit.md").read_text()


def line_of(text, pos):
    return text[:pos].count("\n") + 1


# serial_comm.py without its docstrings: what is left is code and comments.
def without_docstrings(text):
    return re.sub(r'"""(?:.|\n)*?"""', lambda m: "\n" * m.group(0).count("\n"), text)


CODE = without_docstrings(COMM)

items = {}  # kind -> {name: "file:line"}


def add(kind, name, where):
    items.setdefault(kind, {}).setdefault(name, where)


# 1. The regexes the printer's output is read with (named compiled patterns).
for m in re.finditer(r"^(\s*)(regex_[A-Za-z_]+|_temp_command_regex|RESERVED_IDENTIFIER_REGEX)\s*=", CODE, re.M):
    if m.group(2) in ("regex_float_pattern", "regex_positive_float_pattern", "regex_int_pattern"):
        kind = "pattern-text"
    else:
        kind = "regex"
    add(kind, m.group(2), f"serial_comm.py:{line_of(CODE, m.start())}")
for m in re.finditer(r'^\s+"((?:float|int)[A-Z])": re\.compile', CODE, re.M):
    add("regex", f"regexes_parameters.{m.group(1)}", f"serial_comm.py:{line_of(CODE, m.start())}")

# 2. Literal checks on the printer's lines (startswith, in, ==).
READ = re.compile(
    r"""(?:(?:lower_line|line|\w+_line)(?:\.strip\(\))?\.(?:startswith|endswith)\(\s*(["'])(.*?)\1"""
    r"""|(["'])([^"'\n]*)\3\s+(?:not\s+)?in\s+(?:lower_line|line|\w+_line)\b"""
    r"""|(?:lower_line|line)\s*(?:==|!=)\s*(["'])(.*?)\5)"""
)
for m in READ.finditer(CODE):
    lit = (m.group(2) or m.group(4) or m.group(6) or "").strip()
    if lit and lit != ";":
        add("read", lit, f"serial_comm.py:{line_of(CODE, m.start())}")
# the M115 lines, by key
for m in re.finditer(r'data\.get\("([A-Z_]+)"\)', CODE):
    add("read", f"M115:{m.group(1)}", f"serial_comm.py:{line_of(CODE, m.start())}")

# 3. The error lists.
for m in re.finditer(r"^    (_(?:recoverable_communication|resend_request_communication|sd_card|fatal)_errors)\s*=\s*\((.*?)\)\n", CODE, re.M | re.S):
    for s in re.findall(r'"([^"]+)"', m.group(2)):
        add("error", s, f"serial_comm.py:{line_of(CODE, m.start())} ({m.group(1)})")

# 4. Capabilities.
for m in re.finditer(r'^    (CAPABILITY_[A-Z_0-9]+) = "([^"]+)"', CODE, re.M):
    add("capability", m.group(1), f"serial_comm.py:{line_of(CODE, m.start())}")

# 5. Firmware-name rules.
for m in re.finditer(r'"([a-z0-9_-]+)" in firmware_name\.lower\(\)', CODE):
    add("firmware", m.group(1), f"serial_comm.py:{line_of(CODE, m.start())}")

# 6. Action commands.
for m in re.finditer(r'action_name == "([a-z_]+)"', CODE):
    add("action", m.group(1), f"serial_comm.py:{line_of(CODE, m.start())}")

# 7. G-codes sent: literals that start with a G/M/T code and whose rest is only parameters.
sent_forms = {}  # normalised form -> where
BLOCK = re.search(r"^gcodeToEvent = \{.*?^\}", CODE, re.M | re.S)  # a table, nothing sent
doc_end = CODE.find("\ndef gcode_command_for_cmd")  # from there on: examples of parsing
TOKEN = re.compile(r"^[A-Za-z]?(?:\{[^}]*\}|%[sd]|-?\d+(?:\.\d+)?)?$")


def norm(form):
    form = re.sub(r"\{\{\s*([^}]*?)\s*\}\}", r"{\1}", form)
    return form.strip()


def usable(form):
    parts = form.split()
    if not parts or not re.fullmatch(r"[GMT]\d+(?:\.\d+)?", parts[0]):
        return False
    return all(TOKEN.match(p) for p in parts[1:])


for m in re.finditer(r'''[fb]?(["'])((?:[GMT]\d+)[^"'\n]*)\1''', CODE):
    if BLOCK and BLOCK.start() <= m.start() < BLOCK.end():
        continue
    if doc_end != -1 and m.start() > doc_end:
        continue
    form = norm(m.group(2))
    if usable(form) and not form.endswith(":"):
        sent_forms.setdefault(form, f"serial_comm.py:{line_of(CODE, m.start())}")
# the default settings that are commands, and the default gcode scripts
for m in re.finditer(r'^    (helloCommand|sdCancelCommand): str = "([^"]+)"', SCHEMA, re.M):
    sent_forms.setdefault(norm(m.group(2)), f"config_schema.py:{line_of(SCHEMA, m.start())} ({m.group(1)})")
for m in re.finditer(r'^    (\w+): str \| None = (?:\(\s*)?"((?:[^"\\]|\\.)*)"', SCRIPTS, re.M):
    for part in m.group(2).split("\\n"):
        part = norm(re.sub(r"\{%.*?%\}", "", part))
        if usable(part):
            sent_forms.setdefault(part, f"scripts.py:{line_of(SCRIPTS, m.start())} (script {m.group(1)})")
    add("script", f"scripts.gcode.{m.group(1)}", f"scripts.py:{line_of(SCRIPTS, m.start())}")
for m in re.finditer(r'^\s+"(\w+)": "((?:[^"\\]|\\.)*)",', SCRIPTS, re.M):
    for part in re.sub(r"\{%.*?%\}", "\n", m.group(2).replace("\\n", "\n")).split("\n"):
        part = norm(part)
        if part and usable(part):
            sent_forms.setdefault(part, f"scripts.py:{line_of(SCRIPTS, m.start())} (snippet {m.group(1)})")
    add("script", f"scripts.gcode.snippets.{m.group(1)}", f"scripts.py:{line_of(SCRIPTS, m.start())}")
for form, where in sent_forms.items():
    add("sent", form, where)

# 8. What it does when a G-code is queued, sent, queued after sending (per-code handlers).
for m in re.finditer(r"^    def (_gcode_[A-Za-z0-9]+_(?:queuing|sending|sent|queued))\(", CODE, re.M):
    add("handler", m.group(1), f"serial_comm.py:{line_of(CODE, m.start())}")
for m in re.finditer(r"^    def (_atcommand_[a-z_]+)\(", CODE, re.M):
    add("handler", m.group(1), f"serial_comm.py:{line_of(CODE, m.start())}")

# 9. The default settings: every field of the serial connector's settings (config_schema.py),
# with the class it is in; and the fallback defaults in the code (.get("x", 2), x_default = 4.0).
tree = ast.parse(SCHEMA)
classes = {n.name: n for n in tree.body if isinstance(n, ast.ClassDef)}
SECTION_CLASSES = {"SerialTimeoutConfig": "timeout", "SerialMaxTimeouts": "maxCommunicationTimeouts",
                   "SerialCapabilities": "capabilities"}


def fields(cls, prefix):
    for node in cls.body:
        if isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            ann = ast.unparse(node.annotation)
            name = node.target.id
            if ann in SECTION_CLASSES:
                fields(classes[ann], SECTION_CLASSES[ann] + ".")
                continue
            value = ast.unparse(node.value) if node.value is not None else "none"
            add("default", f"{prefix}{name}", f"config_schema.py:{node.lineno} (default {value})")


fields(classes["SerialTimeoutConfig"], "timeout.")
fields(classes["SerialMaxTimeouts"], "maxCommunicationTimeouts.")
fields(classes["SerialCapabilities"], "capabilities.")
fields(classes["SerialConfig"], "")
for m in re.finditer(r'_timeout_intervals\.get\(\s*"(\w+)",\s*([\d.]+)\)|\bget\("(\w+)",\s*(\w+_default)\)|^\s+(\w+_default) = ([\d.]+)', CODE, re.M):
    if m.group(1):
        add("fallback", f"{m.group(1)}={m.group(2)}", f"serial_comm.py:{line_of(CODE, m.start())}")
    elif m.group(5):
        add("fallback", f"{m.group(5)}={m.group(6)}", f"serial_comm.py:{line_of(CODE, m.start())}")
for m in re.finditer(r"^MINIMAL_SD_TIMESTAMP|^STANDARD_BAUDRATES|^BAUDRATE_AUTO|^PORT_AUTO", CODE, re.M):
    add("fallback", m.group(0)[1:] if m.group(0).startswith("^") else m.group(0), f"serial_comm.py:{line_of(CODE, m.start())}")

# 10. The methods that do the talking, and the parsers.
METHODS = re.compile(
    r"^    def (_handle_\w+|_poll_\w+|_set_\w+_interval|_resend\w*|_do_\w*send\w*|_send_from_\w+|_send_loop|"
    r"_get_\w*(?:timer|timeout)\w*|_perform_detection_step|_onConnected|_on_external_reset|_finish_heatup|"
    r"_continue_sending|_trigger_emergency_stop|_emergency_force_send|_needs_checksum|_checksum_enabled|"
    r"_process_command_phase|_process_atcommand_phase|_enqueue_for_sending|_use_up_clear|_readline|"
    r"_trigger_error|_send_firmware_info|_processTemperatures|_monitor|"
    r"sayHello|resetLineNumbers|initSdCard|releaseSdCard|refreshSdFiles|getSdFiles|deleteSdFile|selectFile|"
    r"unselectFile|startPrint|cancelPrint|setPause|startFileTransfer|cancelFileTransfer|fakeOk|"
    r"sendGcodeScript|sendCommand|_sendCommand)\(",
    re.M,
)
for m in METHODS.finditer(CODE):
    add("method", m.group(1), f"serial_comm.py:{line_of(CODE, m.start())}")
for m in re.finditer(r"^def (parse_\w+|process_gcode_line|strip_\w+|canonicalize_temperatures|apply_temperature_offsets|"
                     r"gcode_command_for_cmd|gcode_and_subcode_for_cmd|convert_\w+)\(", CODE, re.M):
    add("method", m.group(1), f"serial_comm.py:{line_of(CODE, m.start())}")

# Marlin's list of codes (reference for the other way round; not asked of the audit).
marlin = set()
letter = None
for n, line in enumerate(MARLIN.splitlines(), 1):
    m = re.match(r"\s*case '([GMTD])':", line)
    if m and n > 350:
        letter = m.group(1)
    for c in re.findall(r"\bcase (\d+):", line) if letter and re.match(r"^\s{6,}", line) else []:
        marlin.add(f"{letter}{c}")
marlin_g, marlin_m = {c for c in marlin if c[0] == "G"}, {c for c in marlin if c[0] == "M"}

# --- what the audit must name -----------------------------------------------------------
missing = {}
for kind, names in items.items():
    if kind == "pattern-text":
        continue
    for name in sorted(names):
        if f"`{name}`" not in AUDIT:
            missing.setdefault(kind, []).append(name)

total = sum(len(v) for k, v in items.items() if k != "pattern-text")
print("OctoPrint items: %d (%s)" % (total, ", ".join(f"{k} {len(v)}" for k, v in items.items() if k != "pattern-text")))
print(f"Marlin dispatch (gcode.cpp): {len(marlin_g)} G-codes, {len(marlin_m)} M-codes, T and D handled")

# Every test the audit names must exist and check the row's G-codes. No row is done another way.
code = "\n".join(p.read_text() for p in (ROOT / "crates").rglob("*.rs"))


def body(name):
    m = re.search(rf"\bfn {name}\(", code)
    if not m:
        return ""
    rest = code[m.start():]
    end = re.search(r"\n    }\n", rest)
    return rest[: end.end()] if end else rest


cited, unproven, loose, weak = set(), [], [], []
unchecked = rows = 0
for row in AUDIT.splitlines():
    if not row.startswith("|") or set(row) <= set("|- "):
        continue
    cells = row.strip().strip("|").split("|")
    if len(cells) < 3 or cells[-1].strip().lower() == "status":
        continue
    rows += 1
    status = cells[-1]
    if "UNCHECKED" in status:
        unchecked += 1
    if re.search(r"other route|another route|another way|in effect|differs|on purpose", status):
        loose.append(cells[0].strip())
    if "test" in status:
        tests = re.findall(r"`([a-z][a-z0-9_]+)`", status)
        cited |= set(tests)
        bodies = "\n".join(body(t) for t in tests)
        for n in re.findall(r"`([^`]+)`", cells[0]):
            if re.match(r"^[GMT]\d", n):
                word = n.split()[0].split("/")[0]
                if word not in bodies:
                    weak.append(f"{cells[0].strip()}: {word} not in {', '.join(tests)}")
    elif re.search(r"\b(same|fixed)\b", status) and "n/a" not in status and "UNCHECKED" not in status:
        unproven.append(cells[0].strip())
no_test = sorted(n for n in cited if not re.search(rf"\bfn {n}\(", code))
print(f"rows in the audit: {rows}; UNCHECKED: {unchecked}; same/fixed without a named test: {len(unproven)}")

failed = False
for title, found in (("rows done another way than OctoPrint (not allowed)", loose),
                     ("rows said same/fixed without a test", unproven),
                     ("named tests that do not check their row's G-code", weak),
                     ("tests named in the audit that do not exist", no_test)):
    if found:
        failed = True
        print(f"{title}: {len(found)}")
        for f in found:
            print(f"  {f}")
if missing:
    failed = True
    print(f"NOT in the audit: {sum(len(v) for v in missing.values())}")
    for kind, names in missing.items():
        print(f"  {kind} ({len(names)}): {', '.join(names)}")
else:
    print("every OctoPrint item is in the audit")

# --- the other way round ----------------------------------------------------------------
WORD = r"(?:[GMTD]\d+(?:\.\d+)?|PRUSA [A-Za-z]+)"


def pattern(form):
    parts = re.split(r"\{[^}]*\}|%[sd]", form)
    return re.compile("^" + ".+".join(re.escape(x) for x in parts) + "$")


theirs = set(sent_forms)
their_patterns = [pattern(f) for f in theirs]


def as_theirs(c):
    probe = re.sub(r"\{[^}]*\}", "1", c)
    return c in theirs or any(p.match(probe) for p in their_patterns)


def nontest(text):
    m = re.search(r"#\[cfg\(test\)\]\s*(?:pub(?:\(crate\))?\s+)?mod tests", text)
    return text[: m.start()] if m else text



def rust_strings(text):
    """The string literals of Rust code with their offsets, comments and char literals skipped."""
    out, i, n = [], 0, len(text)
    while i < n:
        c = text[i]
        if text.startswith("//", i):
            j = text.find("\n", i)
            i = n if j == -1 else j
        elif text.startswith("/*", i):
            j = text.find("*/", i)
            i = n if j == -1 else j + 2
        elif c == "r" and re.match(r'r#*"', text[i:i + 8]) and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            hashes = len(re.match(r"r(#*)", text[i:]).group(1))
            start = i + 1 + hashes + 1
            end = text.find('"' + "#" * hashes, start)
            end = n if end == -1 else end
            out.append((i, text[start:end]))
            i = end + 1 + hashes
        elif c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            out.append((i, text[i + 1:j]))
            i = j + 1
        elif c == "'":
            m = re.match(r"'(?:\\.[^']*|[^\\'])'", text[i:i + 12])
            i += m.end() if m else 1
        else:
            i += 1
    return out

ours = {}
sources = [("agent", p) for p in sorted((ROOT / "crates" / "agent" / "src").glob("*.rs"))]
sources += [("hub", ROOT / "crates" / "hub" / "src" / n) for n in ("sd_card.rs", "main.rs", "queue.rs")]
for crate, path in sources:
    text = nontest(path.read_text())
    for start, literal in rust_strings(text):
        near = text[max(0, start - 3):start] + text[start + len(literal) + 2:start + len(literal) + 5]
        if re.fullmatch(WORD, literal) and "|" in near:
            continue  # a code recognised in a list, not sent
        for part in literal.split("\\n"):
            part = part.strip().replace('\\"', '"')
            if not re.fullmatch(WORD + r"(?:\s.*)?", part):
                continue
            if re.search(r"\d[a-z]\d|[a-z]-?\d+\.\d", part) and not part.startswith(("M117", "M0 ", "M1 ")):
                continue  # an SVG path
            ours.setdefault(part, set()).add(f"{crate}/{path.name}:{line_of(text, start)}")

section = AUDIT.split("## What we send that OctoPrint does not")[1] if "## What we send that OctoPrint does not" in AUDIT else ""
section = re.split(r"\n## ", section)[0]
named = set(re.findall(r"`([^`]+)`", section))
extra = {c: w for c, w in ours.items() if not as_theirs(c) and c not in named}
not_in_marlin = sorted(
    c for c in ours
    if re.match(r"[GM]\d+$", c.split()[0]) and c.split()[0] not in (marlin_g | marlin_m)
)
print(f"G-codes we can send (agent code; hub: sd_card.rs, main.rs, queue.rs): {len(ours)}; "
      f"as OctoPrint sends them: {sum(as_theirs(c) for c in ours)}; not in Marlin's dispatch: {len(not_in_marlin)}")
if not_in_marlin:
    print("  not in Marlin's gcode.cpp (Prusa or host-side): " + ", ".join(not_in_marlin))
if extra:
    failed = True
    print(f"NOT OctoPrint's and not in the section 'What we send that OctoPrint does not': {len(extra)}")
    for c in sorted(extra, key=lambda c: (c.split()[0], c)):
        print(f"  {c}   ({', '.join(sorted(extra[c]))})")
else:
    print("every G-code we send is OctoPrint's, or named in the section 'What we send that OctoPrint does not'")
sys.exit(1 if failed else 0)
