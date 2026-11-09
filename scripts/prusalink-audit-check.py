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

total = sum(len(v) for v in items.values())
print(f"PrusaLink items: {total} ({', '.join(f'{k} {len(v)}' for k, v in items.items())})")
if missing:
    print(f"NOT in the audit: {sum(len(v) for v in missing.values())}")
    for kind, names in missing.items():
        print(f"  {kind}: {', '.join(names)}")
    sys.exit(1)
print("every item is in the audit")
