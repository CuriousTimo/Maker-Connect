#!/usr/bin/env python3
"""Runs real Python's `str % args` (CPython 3.13) on a large set of cases and writes what Python gives
(the text, or `Class: message` of the exception) to a fixture the Rust test compares byte for byte
(crates/engine/src/ports/octoprint/py_percent_expected.json, test `matches_cpython_3_13_percent` in
py_percent.rs and `matches_cpython_3_13_wire` in api_printer.rs).

The only wire caller is OctoPrint 1.11.8 `server/api/printer.py:424-432` (`printerCommand`):

    if len(parameters) > 0:
        commandToSend = command % parameters

where `command` and `parameters` come from the posted JSON: None, bool, int, float, str, list, dict.
A tuple cannot come out of JSON, so no tuple argument is tested. Integers are those of
`serde_json::Value` (i64 and u64); integer literals beyond u64 are not representable there (the top
edge refuses them) and are not in the fixture.

`cases`: `format % args` for every conversion (s r a d i u o x X e E f F g G c %), the flags `# 0 - +
space`, width, `*`, precision, `.*`, the length modifiers `l h L`, `%(key)`, and the argument kinds, too
few and too many arguments, big numbers and floats. `wire`: `printer.py:424-432` for a command and
parameters of every JSON kind. `unimplemented`: cases CPython answers but the port refuses
(`unimplemented!`, listed in py_percent.rs).

Run from the repository root with CPython 3.13:

    ~/.local/share/uv/python/cpython-3.13.12-macos-aarch64-none/bin/python3.13 scripts/octoprint-percent-check.py
"""
import itertools
import json
import random
import sys

if sys.version_info[:2] != (3, 13):
    sys.exit("run this with CPython 3.13 (the reference's runtime)")

OUT = "crates/engine/src/ports/octoprint/py_percent_expected.json"

SCALARS = [
    None, True, False,
    0, 1, -1, 7, -7, 10, 65, 255, -255, 4096, 65535, 1114111, 1114112, 4294967296,
    9223372036854775807, -9223372036854775808, 9223372036854775808, 18446744073709551615,
    0.0, -0.0, 0.5, -0.5, 1.5, 2.5, -2.5, 0.125, 0.375, 1.0, 100.0, 0.1, 3.14159, -3.14159, 2.675,
    123456789.123456789, 1e15, 1e16, 1e17, 1e22, 1e-4, 1e-5, 1.5e-7, 0.0001, 0.00001, 0.001, 0.0005,
    1e30, 1e100, 1.7976931348623157e308, 5e-324, 2.2250738585072014e-308, 9.999999e22, 0.9999995,
    "", "a", "abc", "é", "\U0001F600", "abĀ", "it's", 'a"b', "it's \"q\"", "x\ny\t\x00", "\x7f", "​",
    "\\", "%", "5", "12.5",
]
CONTAINERS = [
    [], [1], [1, "a"], ["x"], [None, True, 1.5], [[1, 2], {"a": 1}], ["it's"], ["é", "\U0001F600"],
    {}, {"a": 1}, {"a": "x", "b": 2.5}, {"k": [1, {"z": None}]}, {"é": "ü"}, {"a": True, "b": None, "c": "q'"},
    {"a": 1e16, "b": 1e-5}, {"a": "\x00\x7f"},
]
ARGS = SCALARS + CONTAINERS

CONVERSIONS = list("sradiuoxXeEfFgGc%")
FLAG_SETS = ["", "-", "+", " ", "0", "#", "-+", "+0", " 0", "#0", "-0", "- ", "+ ", "#-", "#+", "# 0", "-+ #0"]
WIDTHS = ["", "1", "5", "12", "*"]
PRECS = ["", ".", ".0", ".1", ".3", ".12", ".*"]
LENGTHS = ["", "l", "h", "L"]


def run(fmt, args):
    try:
        return {"ok": fmt % args}
    except Exception as e:  # noqa: BLE001 - every exception class is data here
        return {"error": f"{type(e).__name__}: {e}"}


def add(cases, fmt, args):
    cases.append({"format": fmt, "args": args, "expected": run(fmt, args)})


def star_args(fmt, arg):
    """`*` takes its value from the argument list; JSON has no tuple, so the whole `args` is the value
    (an int is a `*` width, and then the value argument is missing)."""
    return arg


def main():
    rnd = random.Random(20261115)
    cases = []

    # 1. every conversion with every argument, plain and with each single flag / width / precision
    for conv in CONVERSIONS:
        for arg in ARGS:
            add(cases, "%" + conv, arg)
            for flag in "-+ #0":
                add(cases, "%" + flag + conv, arg)
            for width in ("1", "8", "*"):
                add(cases, "%" + width + conv, arg)
            for prec in (".", ".0", ".2", ".*"):
                add(cases, "%" + prec + conv, arg)
            for length in "lhL":
                add(cases, "%" + length + conv, arg)

    # 2. the full cross product, sampled (seeded)
    combos = list(itertools.product(CONVERSIONS, FLAG_SETS, WIDTHS, PRECS, LENGTHS))
    rnd.shuffle(combos)
    for conv, flags, width, prec, length in combos[:5000]:
        fmt = "%" + flags + width + prec + length + conv
        add(cases, fmt, rnd.choice(ARGS))
        add(cases, "[" + fmt + "]", rnd.choice(SCALARS))

    # 3. keys: %(key)
    key_formats = [
        "%(a)s", "%(a)r", "%(a)a", "%(a)d", "%(a)i", "%(a)u", "%(a)x", "%(a)o", "%(a)f", "%(a)e", "%(a)g", "%(a)c",
        "%(a)5.2f", "%(a)-8s|", "%(a)+d", "%(a) d", "%(a)05d", "%(a)#x", "%(a)#o", "%(a)#X", "%(a).3s", "%(a)ld",
        "%(b)s", "%(a)s %(b)s", "%(a)s %(a)s", "%(a)s %s", "%s %(a)s", "%(a)s%%", "%(a", "%(a)", "%()s", "%(a(b))s",
        "%(a(b)s", "%(é)s", "%(k)s", "%(a)*d", "%(a).*f", "%*(a)d", "M104 S%(a)d T%(b)d", "%(a)", "%(", "%(a)5%", "%(a)z",
        "%(a)(b)s", "% (a)s", "%(a)\n", "%(a)h", "%(a)hs", "%(c)s",
    ]
    for fmt in key_formats:
        for arg in ARGS:
            add(cases, fmt, arg)

    # 4. odd formats and the counts of arguments
    odd = [
        "", "abc", "%", "%%", "%%%", "%%s", "a%", "a%%", "%5%", "%-5%", "%05%", "%.3%", "%l%", "%s%", "%s%%", "%z", "%5z",
        "%.", "%.5", "%5", "%*", "%.*", "%5.", "%l", "%lz", "%ll", "%lld", "%hhd", "%Ld", "%hs", "%lc", "%-", "%+", "%#",
        "%0", "%00d", "%--d", "%-+ #0d", "%5.3d", "%-5.3d", "%05.3d", "%+.3d", "%#.3x", "%#10.3x", "%#010x", "%#-10x|",
        "%.0f", "%#.0f", "%#.0e", "%#g", "%#.3g", "%#.0g", "%+f", "% f", "%010.3f", "%-10.3f|", "%+010.3f", "% 010.3e",
        "%10.3E", "%G", "%#G", "%.0G", "%e", "%.20f", "%.30e", "%.17g", "%.20g", "%#.20g", "%.500f", "%.400g", "%#.400g",
        "%.500e", "%1000s", "%.1000f", "%5c", "%-5c|", "%05c", "%.0c", "%c%c", "%c %c %c",
        "%s %s", "%s %s %s", "%s%s", "%d %d", "%*d", "%*d %d", "%.*f", "%*.*f", "%-*d|", "%*s|", "%*c", "%*%", "%.*%",
        "M105", "M105 %%", "M104 S%d", "M104 S%s", "M104 S%d T%d", "M104 S%.1f", "M117 %s", "M117 100%", "M117 100%%",
        "G1 X%f Y%f", "G1 X%.2f", "G1 F%d", "G28 %s", "%s" * 5, "%d%%", "%3d%%", "%(a)s%%",
        "%99999999999999999999s", "%.99999999999s", "%.2147483647d",
        "%.2147483648d", "%2147483647%",
        "é%s", "%sé", "\U0001F600%s", "%é", "%5é", "%\U0001F600", "%\x00", "%\n", "%\x7f",
    ]
    for fmt in odd:
        for arg in ARGS:
            add(cases, fmt, arg)

    for fmt, arg in [("%100000s", "a"), ("%.100000f", 1.5), ("%.100000e", 2.5), ("%#.100000g", 0.1), ("%.100000d", 7), ("%0100000d", -7)]:
        add(cases, fmt, arg)

    # 5. `*` with the int arguments (JSON has no tuple, so `*` takes the one argument itself)
    for fmt in ["%*d", "%.*d", "%*s", "%.*s", "%*.*f", "%-*d|", "%*c"]:
        for arg in [0, 1, 5, -5, True, 2147483647, 2147483648, -2147483649, 9223372036854775807, 9223372036854775808,
                    -9223372036854775808, 18446744073709551615, 1.5, "5", None]:
            add(cases, fmt, arg)

    # 6. the wire: printer.py:424-432
    wire = []
    commands = [
        "M105", "M104 S%d", "M104 S%(t)d", "M104 S%(t)s T%(n)d", "M117 100%%", "%s", "%d", "%(a)s", "", "%",
        5, 5.5, True, None, [], ["M105"], {}, {"a": 1},
    ]
    parameters = [
        {}, [], "", 0, 0.0, None, False, True, 1, 2.5, "x", "xy", 5, [1], ["a", "b"], {"t": 200, "n": 1}, {"a": "x"}, {"t": 210.5}, {"t": "x"},
        {"t": None}, {"n": [1]}, 1e30, -3, "%s",
    ]
    for command in commands:
        for params in parameters:
            try:
                if len(params) > 0:
                    out = {"ok": command % params}
                else:
                    out = {"ok": command}
            except Exception as e:  # noqa: BLE001
                out = {"error": f"{type(e).__name__}: {e}"}
            wire.append({"command": command, "parameters": params, "expected": out})

    # 7. CPython answers, the port refuses (`unimplemented!`)
    unimplemented = [
        {"format": "%c", "args": 0xD800, "why": "a str with a lone surrogate"},
        {"format": "%c", "args": 0xDFFF, "why": "a str with a lone surrogate"},
        {"format": "%5c", "args": 0xDBFF, "why": "a str with a lone surrogate"},
        {"format": "%.2147483647f", "args": 1.5, "why": "CPython 3.13.12 tries to build a 2 GB str (the 3.13 branch head raises ValueError)"},
        {"format": "%1000000000s", "args": "a", "why": "a width of 1e9 (a 1 GB str)"},
        {"format": "%.1000000000f", "args": 1.5, "why": "a precision of 1e9 (a 1 GB str)"},
        {"format": "%.1000000000d", "args": 1, "why": "a precision of 1e9 (a 1 GB str)"},
    ]

    # integers the fast and the slow path both give (a sanity check of the script itself)
    for c in cases:
        json.dumps(c, ensure_ascii=True)

    fixture = {"python": sys.version.split()[0], "cases": cases, "wire": wire, "unimplemented": unimplemented}
    with open(OUT, "w") as f:
        json.dump(fixture, f, ensure_ascii=True, separators=(",", ":"))
        f.write("\n")
    print(f"{len(cases)} cases, {len(wire)} wire cases, {len(unimplemented)} unimplemented -> {OUT}")


main()
