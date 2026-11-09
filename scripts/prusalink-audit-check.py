#!/usr/bin/env python3
"""Checks docs/prusalink-audit.md against the PrusaLink source, mechanically.

Every item below is taken from the source in reference/Prusa-Link, never typed by hand:
patterns the printer's output is read with, polled values, EEPROM addresses, G-codes sent,
commands PrusaLink takes, web controls, handlers of the printer's own messages. Each must be
named in the audit (in backticks). Prints what is missing; exits 1 if anything is.

Run: python3 -I scripts/prusalink-audit-check.py
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LINK = ROOT / "reference" / "Prusa-Link" / "prusa" / "link"
AUDIT = (ROOT / "docs" / "prusalink-audit.md").read_text()


def read(rel):
    return (LINK / rel).read_text()


items = {}  # kind -> set of names

# Patterns the printer's output is read with.
items["pattern"] = set(re.findall(r"^([A-Z0-9_]+_REGEX|LFN_CAPTURE|TM_ERROR_CLEARED|URLS_FOR_WIZARD)\s*=",
                                  read("printer_adapter/structures/regular_expressions.py"), re.M))
# Values PrusaLink polls or keeps (its WatchedItems).
items["polled"] = set(re.findall(r'WatchedItem\(\s*"([a-z_]+)"', read("printer_adapter/printer_polling.py")))
# EEPROM addresses it reads or writes.
eeprom = read("printer_adapter/structures/model_classes.py")
block = eeprom[eeprom.index("class EEPROMParams"):].split("\n\n\n")[0]
items["eeprom"] = set(re.findall(r"^\s+([A-Z_]+) = 0x", block, re.M))
# G-codes it sends: the first word of every literal that starts with a G-, M- or D-code.
sent = set()
for path in LINK.rglob("*.py"):
    for literal in re.findall(r"""[fb]?["']((?:[GMD]\d+(?:\.\d+)?|PRUSA [A-Za-z]+|;C32u2_RM[DE])[^"'\\]*)""", path.read_text()):
        word = literal.split()[0] if not literal.startswith("PRUSA") else " ".join(literal.split()[:2])
        if not re.fullmatch(r"[GMD]\d+(\.\d+)?|PRUSA [A-Za-z]+|;C32u2_RM[DE]", word):
            continue
        sent.add(word)
items["sent"] = sent
# Commands it takes (from Connect, its web, its own parts): command_name of each Command.
items["command"] = set(re.findall(r'command_name = "([a-z_]+)"', read("printer_adapter/command_handlers.py")))
# Commands it takes from Prusa Connect (set_handler in prusa_link.py).
items["connect"] = set(re.findall(r"set_handler\(\s*CommandType\.([A-Z_]+)", read("printer_adapter/prusa_link.py")))
# Web controls (functions in web/controls.py taking a request).
items["control"] = set(re.findall(r"^def ([a-z_]+)\(req", read("web/controls.py"), re.M))
# Handlers of the printer's own messages (special_commands.py and add_handler calls).
handlers = set()
for path in (LINK / "printer_adapter").rglob("*.py"):
    handlers |= set(re.findall(r"add_(?:decoupled_)?handler\(\s*([A-Z0-9_]+)", path.read_text()))
items["handled"] = handlers

missing = {}
for kind, names in items.items():
    for name in sorted(names):
        if f"`{name}`" not in AUDIT:
            missing.setdefault(kind, []).append(name)

# Every test the audit names must exist as a function in the code.
code = "\n".join(p.read_text() for p in (ROOT / "crates").rglob("*.rs"))
cited = set()
unproven = []
for row in AUDIT.splitlines():
    if not row.startswith("|"):
        continue
    status = row.rstrip("|").split("|")[-1]
    if "test" in status:
        cited |= {n for n in re.findall(r"`([a-z][a-z0-9_]+)`", status)}
    elif re.search(r"\b(same|fixed)\b", status) and "n/a" not in status:
        unproven.append(row.split("|")[1].strip())
# To the letter (the user's rule): a row may not be done "another way". Each row whose status
# says so is not done.
loose = []
for row in AUDIT.splitlines():
    if row.startswith("|"):
        status = row.rstrip("|").split("|")[-1]
        if re.search(r"other route|another route|in effect|differs|on purpose", status):
            loose.append(row.split("|")[1].strip())
no_test = sorted(n for n in cited if not re.search(rf"\bfn {n}\(", code))

# A test named for a row must check what the row names: each G-code or printer text in the
# row's first cell (in backticks) appears in that test's body.
def body(name):
    m = re.search(rf"\bfn {name}\(", code)
    if not m:
        return ""
    rest = code[m.start():]
    end = re.search(r"\n    }\n", rest)
    return rest[: end.end()] if end else rest
weak = []
for row in AUDIT.splitlines():
    if not row.startswith("|"):
        continue
    cells = row.strip("|").split("|")
    status = cells[-1]
    tests = re.findall(r"`([a-z][a-z0-9_]+)`", status) if "test" in status else []
    if not tests:
        continue
    names = [
        n for n in re.findall(r"`([^`]+)`", cells[0])
        if re.match(r"^(?:[GMD]\d|PRUSA|;)", n) and not n.endswith("_REGEX")
    ]
    bodies = "\n".join(body(t) for t in tests)
    for n in names:
        word = n.split()[0].split("/")[0]
        if word not in bodies:
            weak.append(f"{cells[0].strip()}: {word} not in {', '.join(tests)}")
    # A row about what the printer says (a pattern): at least one of the printer's texts the
    # row quotes (its second cell, in backticks) appears in the tests, as the printer says it.
    if re.search(r"_REGEX|LFN_CAPTURE|TM_ERROR_CLEARED", cells[0]) and len(cells) > 2:
        # A text with "…" (left out) counts by its longest part.
        texts = [max(t.split("…"), key=len).strip() for t in re.findall(r"`([^`]+)`", cells[1])]
        texts = [t for t in texts if len(t) >= 4]
        plain = bodies.replace('\\"', '"')
        if texts and not any(t in plain for t in texts):
            weak.append(f"{cells[0].strip()}: none of {texts[:3]} in {', '.join(tests)}")

total = sum(len(v) for v in items.values())
print(f"PrusaLink items: {total} ({', '.join(f'{k} {len(v)}' for k, v in items.items())})")
print(f"rows same/fixed without a named test (not proven): {len(unproven)}")
if loose:
    print(f"rows done another way than PrusaLink (not allowed): {len(loose)}")
    for name in loose:
        print(f"  {name}")
if weak:
    print(f"named tests that do not check their row's G-code: {len(weak)}")
    for w in weak:
        print(f"  {w}")
    sys.exit(1)
if no_test:
    print(f"tests named in the audit that do not exist: {', '.join(no_test)}")
    sys.exit(1)
if missing:
    print(f"NOT in the audit: {sum(len(v) for v in missing.values())}")
    for kind, names in missing.items():
        print(f"  {kind}: {', '.join(names)}")
    sys.exit(1)
print("every item is in the audit")
if loose:
    sys.exit(1)

# The other way round: every G-code our agent and hub can send (literals in their code, test
# code left out) is one PrusaLink sends, in its form ({...} in either matches any value), or
# is named in the audit's section "What we send that PrusaLink does not" with how it is kept
# from Prusa's MK3 family. The printer must not tell us from PrusaLink.
WORD = r"(?:[GMD]\d+(?:\.\d+)?|PRUSA [A-Za-z]+)"
def first_word(code):
    return " ".join(code.split()[:2]) if code.startswith("PRUSA") else code.split()[0]
theirs = set()
for path in LINK.rglob("*.py"):
    # Adjacent literals are one string in Python ("M73 Q… " "P… R…").
    text = re.sub(r"""(["'])\s*\n\s*[fb]?\1""", "", path.read_text())
    for m in re.finditer(r"""[fb]?(["'])((?:[GMD]\d+(?:\.\d+)?|PRUSA [A-Za-z]+)(?:(?!\1)[^\\]|\\.)*)\1""", text):
        before = text[max(0, m.start() - 60):m.start()].split("\n")[-1]
        if "compile" not in before and "REGEX" not in before:
            # As sent: \" is ", \x7E is ~.
            theirs.add(m.group(2).strip().replace('\\"', '"').replace("\\x7E", "~"))
def pattern(form):
    parts = re.split(r"\{[^}]*\}", form)
    return re.compile("^" + ".+".join(re.escape(x) for x in parts) + "$")
their_patterns = [pattern(f) for f in theirs]
def nontest(text):
    m = re.search(r"#\[cfg\(test\)\]\s*(?:pub(?:\(crate\))?\s+)?mod tests", text)
    return text[: m.start()] if m else text
ours = {}
for crate in ("agent", "hub"):
    for path in sorted((ROOT / "crates" / crate / "src").glob("*.rs")):
        text = nontest(path.read_text())
        for m in re.finditer(r'"((?:[^"\\]|\\.)*)"', text):
            literal = m.group(1)
            near = text[max(0, m.start() - 3):m.start()] + text[m.end():m.end() + 3]
            if re.fullmatch(WORD, literal) and "|" in near:
                continue  # a code recognised in a list, not sent
            for part in literal.split("\\n"):
                part = part.strip().replace('\\"', '"')
                if not re.fullmatch(WORD + r"(?:\s.*)?", part):
                    continue
                if re.search(r"\d[a-z]\d|[a-z]-?\d+\.\d", part) and not part.startswith(("M117", "M0 ", "M1 ")):
                    continue  # an SVG path
                line = text[: m.start()].count("\n") + 1
                ours.setdefault(part, set()).add(f"{crate}/{path.name}:{line}")
section = AUDIT.split("## 6.")[1] if "## 6." in AUDIT else ""
named = set(re.findall(r"`([^`]+)`", section))
def as_theirs(code):
    probe = re.sub(r"\{[^}]*\}", "1", code)
    return code in theirs or any(p.match(probe) for p in their_patterns)
extra = {c: w for c, w in ours.items() if not as_theirs(c) and c not in named}
print(f"G-codes we can send: {len(ours)}; as PrusaLink sends them: {sum(as_theirs(c) for c in ours)}")
if extra:
    print(f"NOT PrusaLink's and not in section 6 of the audit: {len(extra)}")
    for code in sorted(extra, key=lambda c: (first_word(c), c)):
        print(f"  {code}   ({', '.join(sorted(extra[code]))})")
    sys.exit(1)
print("every G-code we send is PrusaLink's, or kept from the MK3 family (section 6)")

# PrusaLink's own lines on the printer's screen (lcd_printer.py): every text it shows (its
# string literals, docstrings left out) is in the agent's port (crates/agent/src/lcd.rs, its
# comments left out), or is named in the audit's section 7 with why not (a dictionary key, a
# log line, a text the agent shows otherwise).
LCD_LITERAL = re.compile(r'(?<![\w])f?"((?:[^"\\]|\\.)*)"')
lcd_py = (LINK / "printer_adapter" / "lcd_printer.py").read_text()
lcd_py = re.sub(r'"""(?:.|\n)*?"""', "", lcd_py)
lcd_rs = "\n".join(
    l for l in (ROOT / "crates" / "agent" / "src" / "lcd.rs").read_text().splitlines()
    if not l.strip().startswith("//")
)
section7 = AUDIT.split("## 7.")[1] if "## 7." in AUDIT else ""
named7 = set(re.findall(r"`([^`]+)`", section7))
lcd_texts = list(dict.fromkeys(LCD_LITERAL.findall(lcd_py)))
missing_lcd = []
for literal in lcd_texts:
    text = literal.replace("\\x7E", "~")
    parts = [p for p in re.split(r"\{[^}]*\}", text) if p]
    if literal in named7 or text in named7 or all(p in lcd_rs for p in parts):
        continue
    missing_lcd.append(literal)
print(f"PrusaLink's screen texts: {len(lcd_texts)}; not in lcd.rs nor in section 7: {len(missing_lcd)}")
if missing_lcd:
    for literal in missing_lcd:
        print(f"  {literal!r}")
    sys.exit(1)

# The test that runs an MK3 session (an_mk3_is_sent_only_what_prusalink_sends) checks what is
# sent against a list of PrusaLink's forms: each must be one of PrusaLink's, as it writes it.
marlin_rs = (ROOT / "crates" / "agent" / "src" / "marlin.rs").read_text()
listed = re.search(r"PRUSALINK_SENDS: &\[&str\] = &\[(.*?)\];", marlin_rs, re.S).group(1)
forms = [f.replace('\\"', '"') for f in re.findall(r'"((?:[^"\\]|\\.)*)"', listed)]
shape = lambda f: re.sub(r"\{[^}]*\}", "{}", f)
their_shapes = {shape(f) for f in theirs}
not_theirs = [f for f in forms if shape(f) not in their_shapes and not as_theirs(f)]
print(f"forms in the session test's list: {len(forms)}; PrusaLink's: {len(forms) - len(not_theirs)}")
if not_theirs:
    print(f"NOT PrusaLink's: {', '.join(not_theirs)}")
    sys.exit(1)
