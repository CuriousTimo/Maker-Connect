#!/usr/bin/env python3
"""Runs real Python's `str.format` (CPython 3.13) on the calls `py_format.rs` has to answer, and writes
what Python gives (the text, or the exception class) to a fixture the Rust test compares byte for byte
(crates/engine/src/ports/octoprint/py_format_expected.json). It also writes the code points that are
not `str.isprintable()` (crates/engine/src/ports/octoprint/py_nonprintable.txt), which `repr()` needs, and
the attribute names of a str and of None (py_str_attrs.txt).

The calls are those of OctoPrint 1.11.8 `util/comm.py:3562-3592` (`_process_registered_message`):

    output = template.format(*match.groups())          # str / None arguments, positional
    except KeyError: output = template.format(**match.groupdict())   # str / None, named

plus every call the unit tests of py_format.rs make. Run from the repository root with CPython 3.13:

    ~/.local/share/uv/python/cpython-3.13.12-macos-aarch64-none/bin/python3.13 scripts/octoprint-format-check.py
"""
import itertools
import json
import re
import sys
import unicodedata

if sys.version_info[:2] != (3, 13):
    sys.exit("run this with CPython 3.13 (the reference's runtime)")

OUT = "crates/engine/src/ports/octoprint/py_format_expected.json"
OUT_NONPRINTABLE = "crates/engine/src/ports/octoprint/py_nonprintable.txt"
OUT_ATTRS = "crates/engine/src/ports/octoprint/py_str_attrs.txt"

ARGS = [
    [],
    ["a"],
    ["ab"],
    ["abc"],
    ["21.5"],
    [None],
    ["a", "b"],
    ["ab", "4"],
    ["a", None],
    [None, "b"],
    ["a", "b", "c"],
    ["", "x"],
    ["éä"],
    ["\U0001F600x"],
    ["it's"],
    ['it\'s "q"'],
    ["a\nb\t\x00"],
    ["a b"],
    ["­​ ͸ "],
    ["\\"],
]
KWARGS = [
    [],
    [["name", "v"]],
    [["name", None]],
    [["a", "x"], ["b", None], ["c", "z"]],
    [["group1", "1"], ["group2", None]],
    [["x", "ab"]],
    [["w", "6"], ["x", "ab"]],
]

TEMPLATES = []


def add(*templates):
    TEMPLATES.extend(templates)


# --- the shapes of a `control` template (comm.py:3562-3592): positional, named, plain text
add(
    "", "plain", "Temp {0}C", "{}-{}", "{1} {0} {1}", "{{x}}", "{{}}", "{{{0}}}", "{0}{0}{0}", "{0}", "{}", "{} {}", "{} {} {}",
    "{name}", "{name}{name}", "{a}-{b}-{c}", "{group1}/{group2}", "{x}", "{0} {name}", "{} {name}", "{name} {0}",
    "{0}\n{1}", "T: {0}°C", "{é}", "{0}}}", "{{{{", "}}", "{{", "{{0}}", "{{{{0}}}}",
    "{0:}", "{0!s}", "{0!r}", "{0!a}", "{!s}", "{!r}", "{!a}", "{x!r}", "{x!s}", "{x!a}", "{name!r}",
)
# --- errors in the syntax
add(
    "{", "}", "{0", "a } b", "a { b", "{0!}", "{0!x}", "{0!rx}", "{0!r", "{0!r:", "{0!r:}", "{0!r:5}", "{0!:}", "{0:",
    "{0:{", "{0:{}", "{0:{}}", "{0:{1}}", "{0:{1}{2}}", "{0:{{}}}", "{0:{{1}}}", "{0:}}", "{0:{0:{1}}}", "{:{}}", "{:{}{}}", "{ }", "{ 0}",
    "{0 }", "{0 !r}", "{-1}", "{+1}", "{00}", "{01}", "{1.5}", "{0x1}", "{١}", "{1_0}", "{99999999999999999999}",
    "{\u0661\u0660}", "{0[\u0661]}", "{0:\u0665}|", "{0:.\u0662}|", "{0:\u0661\u0662}|", "{0:\u0665.\u0662}|", "{0:\u00b2}|", "{\u00b2}", "{0[\u00b2]}",
    "{0[}", "{0]}", "{0[0]", "{0[0]x}", "{0[0]}", "{0[1]}", "{0[5]}", "{0[x]}", "{0[-1]}", "{0[ ]}", "{0[]}", "{0.}", "{0..x}", "{.x}",
    "{[0]}", "{[}", "{0[0][0]}", "{0[0].x}", "{0.nope}", "{0.x.y}", "{x.nope}", "{x[0]}", "{x[9]}", "{x[a]}",
    "{0!r!s}", "{0!!}", "{!}", "{!:}", "{:}", "{::}", "{:::}", "{0:!r}", "{0:x!r}",
    "{0:5!r}", "{0:{!r}}", "{0:[}", "{0:]}", "{x:{w}}", "{x:{w}.{w}}", "{x:{missing}}", "{x:{0}}", "{0:{x}}",
)
# --- attributes (Python accepts them; py_format cannot print what they print)
add(
    "{0.upper}", "{0.__class__}", "{0.__class__.__name__}", "{0.__doc__}", "{0.__len__}", "{0.__hash__}",
    "{x.upper}", "{x.__class__.__name__}", "{0.format}", "{0.strip}", "{0.__str__}",
)
# --- conversions
for c in "sra":
    add("{0!%s}" % c, "{0!%s:>6}" % c, "{0!%s:6}" % c, "{0!%s:.1}" % c, "{x!%s}" % c)
add("{0!d}", "{0!S}", "{0!R}", "{0!b}", "{0!é}", "{0!rr}", "{0!r }", "{0!s }")
# --- format specs on a str / None argument: every combination of the standard mini-language that
# can be told apart (fill+align, sign, #, 0, width, grouping, precision, type)
fills = ["", "*", "0", " ", "é", "{{"]
aligns = ["", "<", ">", "^", "="]
signs = ["", "+", "-", " "]
alts = ["", "#"]
zeros = ["", "0"]
widths = ["", "1", "5", "12"]
groupings = ["", ",", "_"]
precisions = ["", ".0", ".1", ".2", ".10"]
types = ["", "s", "d", "r", "x", "f", "e", "g", "n", "%", "c", "b", "o", "X", "E", "G", "F", "S"]  # one type character at a time, below

specs = set()
for align, sign, alt, zero, grouping, typ in itertools.product(aligns, signs, alts, zeros, groupings, ["", "s", "d", "r", "x", "%"]):
    specs.add(f"{align}{sign}{alt}{zero}5{grouping}{typ}")
for fill, align, width, precision, typ in itertools.product(fills, aligns, widths, precisions, [""]):
    if fill and not align:
        continue
    specs.add(f"{fill}{align}{width}{precision}{typ}")
for typ in types:
    for prefix in ["", "5", ".2", "<5", "+", "#", "0", ",", "_"]:
        specs.add(f"{prefix}{typ}")
for spec in ["0", "00", "05", "005", "5.", ".", "..", ".x", "5.x", "1.2.3", "<<", "<<<", "**^", "*^*5", ">>5", "x", "ss", "s s", " s", "s ", "5s", "5.2s",
             "é", "<é", "éé<5", "99999999999999999999", "5.99999999999999999999", "{}", "{0}", "{1}", "{x}", "{w}", "{0}{1}",
             ">{}", "{}.{}", ".{}", "<{0}", "^{1}.{0}"]:
    specs.add(spec)
for spec in sorted(specs):
    add("{0:%s}|" % spec, "{x:%s}|" % spec)

# --- the format spec that holds a nested field
add("{0:{1}}", "{0:>{1}}", "{0:{1}.{1}}", "{0:{1}{1}}", "{0:{1}}|{1}", "{:{}}", "{0:{x}}", "{x:{w}}", "{x:>{w}}", "{x:{w}.{w}}")

CASES = []
seen = set()


def add_case(t, args, kwargs):
    key = (t, json.dumps(args), json.dumps(kwargs))
    if key not in seen:
        seen.add(key)
        CASES.append((t, args, kwargs))


# a template is run with every positional argument list, every keyword set, and a few of both
# (comm.py:3562 gives it one or the other: `format(*groups)`, and `format(**groupdict)` on a KeyError)
SPEC_TEMPLATE = re.compile(r"^\{(0|x):[^{}!]*\}\|$")
REP_ARGS = [["ab"], [None]]
REP_KWARGS = [[["x", "ab"]], [["x", None]]]
for t in TEMPLATES:
    if SPEC_TEMPLATE.match(t):
        # the many specs: two representative arguments each
        if t.startswith("{0:"):
            for args in REP_ARGS:
                add_case(t, args, [])
        else:
            for kwargs in REP_KWARGS:
                add_case(t, [], kwargs)
        continue
    for args in ARGS:
        add_case(t, args, [])
    for kwargs in KWARGS:
        add_case(t, [], kwargs)
    for args, kwargs in [(["a", "b"], [["name", "v"]]), (["ab"], [["x", "ab"], ["w", "6"]]), ([None], [["name", None]])]:
        add_case(t, args, kwargs)

# the `repr` / `ascii` of every kind of character
CHARS = [
    "\x00", "\x07", "\t", "\n", "\r", "\x1b", "\x7f", "\x80", "\x9f", "\xa0", "\xa1", "\xad", "\xff", "'", '"', "\\", "Ā", "͸", "Ϳ",
    "؀", "؜", "ก", " ", "᠎", " ", " ", "​", "‍", "‎", " ", " ", "‪", " ",
    "⁠", "⁦", "　", "퟿", "", "", "﷐", "﻿", "￹", "�", "￿", "\U00010000", "\U0001F600",
    "\U000E0001", "\U000E0100", "\U000F0000", "\U0010FFFF", "é", "́", "é", "中", "\U00020000",
]
for ch in CHARS:
    for conv in "ra":
        CASES.append(("{0!%s}" % conv, [ch], []))
        CASES.append(("{0!%s:>5}" % conv, [ch], []))
    CASES.append(("{0:>3}|{0:<3}|{0:^3}|{0:.1}", [ch], []))
for a, b in itertools.product(["'", '"', "a'", 'a"', "'\"", "\\'", "a\\"], repeat=2):
    CASES.append(("{0!r}", [a + b], []))
CASES.append(("{0!r}", ["it's"], []))

# every field of the unit tests in py_format.rs (they are in the lists above; these are the exact calls)
for t, args, kwargs in [
    ("Temp {0}C", ["21.5"], []), ("{}-{}", ["a", "b"], []), ("{1} {0} {1}", ["a", "b"], []), ("{{x}}", [], []), ("{0}", [None], []),
    ("{} {0}", ["a"], []), ("{0} {}", ["a"], []), ("{1}", ["a"], []), ("{}", [], []), ("{name}", [], []), ("{name}", [], [["name", "v"]]),
    ("{name}", [], [["name", None]]), ("{0!r}", ["ab"], []), ("{0!s}", ["ab"], []), ("{0:5}|", ["ab"], []), ("{0:>5}|", ["ab"], []),
    ("{0:^5}|", ["ab"], []), ("{0:*^5}", ["ab"], []), ("{0:.1}", ["ab"], []), ("{0:05}", ["ab"], []), ("{0:{1}}", ["ab", "4"], []),
    ("{0:d}", ["ab"], []), ("{0:+}", ["ab"], []), ("{0:=5}", ["ab"], []), ("{0:5}", [None], []), ("{0!s:6}", [None], []),
    ("{0[1]}", ["abc"], []), ("{0[5]}", ["abc"], []), ("{0[x]}", ["abc"], []), ("{0.nope}", ["abc"], []),
]:
    CASES.append((t, args, kwargs))


def run(template, args, kwargs):
    try:
        if kwargs and not args:
            out = template.format(**{k: v for k, v in kwargs})
        else:
            out = template.format(*args, **{k: v for k, v in kwargs})
    except Exception as e:  # the class is what matters
        return {"error": type(e).__name__}
    return {"ok": out}




def uses_attribute(template):
    # `{0.upper}`: a `.name` in the field name (before any `!` or `:`)
    for m in re.finditer(r"\{([^{}!:]*)", template):
        if re.search(r"\.[^.\[\]]", m.group(1)):
            return True
    return False


out = []
for template, args, kwargs in CASES:
    expected = run(template, args, kwargs)
    case = {"template": template, "args": args, "kwargs": kwargs}
    if "ok" in expected and uses_attribute(template):
        # Python accepts an attribute of a str / None as a field; py_format cannot print what that prints
        # (a repr with a memory address, or the object's class / docs): `unimplemented!` there, listed
        case["unimplemented"] = "attribute of a str/None as a field"
    else:
        case["expected"] = expected
    out.append(case)

with open(OUT, "w", encoding="utf-8") as f:
    json.dump({"python": sys.version.split()[0], "unicode": unicodedata.unidata_version, "cases": out}, f, ensure_ascii=True, separators=(",", ":"))
    f.write("\n")
print(f"{len(out)} cases written to {OUT}")
print("errors:", sum(1 for c in out if "expected" in c and "error" in c["expected"]), "unimplemented:", sum(1 for c in out if "unimplemented" in c))

# the code points `str.isprintable()` is False for, as ranges
ranges = []
start = None
for cp in range(0x110000):
    np_ = not chr(cp).isprintable()
    if np_ and start is None:
        start = cp
    if not np_ and start is not None:
        ranges.append((start, cp - 1))
        start = None
if start is not None:
    ranges.append((start, 0x10FFFF))
with open(OUT_NONPRINTABLE, "w") as f:
    f.write(f"# str.isprintable() is False, Python {sys.version.split()[0]}, Unicode {unicodedata.unidata_version}: first-last (hex)\n")
    for a, b in ranges:
        f.write(f"{a:X}-{b:X}\n")
print(f"{len(ranges)} non-printable ranges written to {OUT_NONPRINTABLE}")

# the attribute names a `str` and `None` have (`{0.name}` finds exactly these: dir())
with open(OUT_ATTRS, "w") as f:
    f.write(f"# dir('') and dir(None), Python {sys.version.split()[0]}: kind name\n")
    for kind, obj in (("str", ""), ("None", None)):
        for name in dir(obj):
            f.write(f"{kind} {name}\n")
print("attribute names written to", OUT_ATTRS)
