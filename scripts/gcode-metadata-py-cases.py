#!/usr/bin/env python3
"""Record what CPython does for the built-ins `crates/gcode-metadata/src/py.rs` ports.

Writes crates/gcode-metadata/tests/py-cases.json: per case the input and what CPython gave,
the result as text or the exception's class and message. `tests/py_cases.rs` runs ours on
the same inputs and wants the same. `--check` fails when the file is not what this Python
records.

    reference/.venv-connect-sdk-313/bin/python -I scripts/gcode-metadata-py-cases.py [--check]
"""

import json
import random
import sys
from pathlib import Path

OUT = (Path(__file__).resolve().parent.parent / "crates" / "gcode-metadata" / "tests" /
       "py-cases.json")


def outcome(call):
    """('ok', text) or (class name, message)"""
    try:
        return ["ok", call()]
    except Exception as err:  # pylint: disable=broad-except
        return [type(err).__name__, str(err)]


def no_surrogates(text):
    return not any(0xd800 <= ord(c) <= 0xdfff for c in text)


def number_strings(rng):
    """Texts for int() and float(): hand-picked, then random ones from the same pieces."""
    hand = [
        "0", "12", "-0", "+7", "0012", " 12 ", "\t12\n", "1_000", "1__0", "_1", "1_", "+_1",
        "12.0", "1e3", "", " ", "-", "+", "abc", "12a", "\x0012", "12\x00", "1_0\x005",
        "١٢", "１２", "12 ", " 12", "1 ", "\x1c12\x1f",
        "12é", "é12", "1\x7f", "0x10", "1.", ".5", "5.", ".", "e5", "1e", "1e+",
        "1e-3", "1E5", "1e5000", "-1e5000", "1e-5000", "inf", "-inf", "+inf", "Infinity",
        "INFINITY", "infinit", "infx", "nan", "-nan", "NaN", "+NAN", "nanx", "1_0.5",
        "1_0.5_0", "1._5", "1_.5", "1e1_0", "1_e10", "0.1", "0.2", "2.675", "1.005",
        "123456789012345678901234567890", "-0.0", "4.35", "1e16", "1e-5", "0.0001",
        "  -3.25  ", "3.25e2", "٣.٥", "1" * 4300, "1" * 4301, "0" * 4301,
        "1_" * 4300 + "1", "9" * 400 + ".5", "\n", "1\n2", "١_٢",
    ]
    pieces = ["0", "1", "9", "5", "_", ".", "e", "E", "-", "+", " ", "\t", "١",
              " ", "n", "a", "i", "f", "\x00"]
    random_texts = ["".join(rng.choice(pieces) for _ in range(rng.randint(1, 8)))
                    for _ in range(3000)]
    return hand + random_texts


def floats(rng):
    hand = [0.0, -0.0, 1.0, -1.5, 0.1, 0.25, 0.35, 2.675, 1.005, 0.125, 0.375, 2.5, 3.5,
            1e16, 1e15, 9999999999999998.0, 1e-4, 1e-5, 123.456, 1.7976931348623157e308,
            5e-324, -2.5e-7, 17.145, 17.155, 0.005, 0.015, 0.025, 1234567.125,
            float("inf"), float("-inf"), float("nan"), 2.0**53, 2.0**64, 2.0**200, -2.0**70,
            12345678901234567890.0, 0.49999999999999994]
    random_floats = []
    for _ in range(3000):
        kind = rng.randint(0, 3)
        if kind == 0:
            random_floats.append(rng.uniform(-1000, 1000))
        elif kind == 1:
            random_floats.append(round(rng.uniform(-100, 100), rng.randint(1, 5)))
        elif kind == 2:
            random_floats.append(rng.randint(-10**6, 10**6) / 2**rng.randint(0, 12))
        else:
            random_floats.append(rng.uniform(-1, 1) * 10.0**rng.randint(-30, 300))
    return hand + random_floats


def strings(rng):
    hand = ["", "a", "it's", 'say "hi"', "both ' and \"", "back\\slash", "tab\tnl\nret\r",
            "\x00\x1f\x7f", " éÿ", "Ā 　", "\U0001f600",
            "\U000e0001", "﻿", "͸", "PLA;PETG"]
    alphabet = [chr(c) for c in list(range(0, 0x250)) + list(range(0x2000, 0x2070)) +
                [0x3000, 0xfeff, 0xfffd, 0x1f600, 0xe0001, 0x10ffff]]
    random_strings = ["".join(rng.choice(alphabet) for _ in range(rng.randint(0, 6)))
                      for _ in range(2000)]
    return hand + random_strings


def json_docs(rng):
    hand = [
        "", " ", "null", "true", "false", "NaN", "Infinity", "-Infinity", "-Inf", "nul",
        "0", "-0", "01", "1.", "1.5", "-1.5e3", "1e", "1e5", "1E+5", "1e-5", "2.0E", "-",
        "123456789012345678901234567890", "1" * 4301, '"x"', '"a\\nb"', '"\\u00e9"',
        '"\\ud83d\\ude00"', '"\\ud83d"', '"\\ude00"', '"\\ud83d\\u0041"', '"\\u12"',
        '"\\uZZZZ"', '"\\x"', '"abc', '"a\tb"', '"\\"', '[]', '[1,2]', '[1,]', '[1 2]',
        '[', ']', '{}', '{"a": 1}', '{"a" 1}', '{"a": 1,}', '{"a": 1, "a": 2, "b": 3}',
        '{a: 1}', '{"a": [1, {"b": null}]}', ' [ 1 , 2 ] ', '[1] x', '﻿[1]',
        '{"objects": [{"name": "Shape-Box id:0 copy 0", "polygon": [[111.000, 111.000]]}]}',
        '[1e400, -1e400, 1e-400]', '[0.1, 0.30000000000000004, 1e16, 1e-7]',
        '"é\U0001f600"', '{"k\\u00e9y": "v"}', '[true,false,null]', '[ ]', '{ }',
        '[NaN, Infinity, -Infinity]', '"\x7f"', "[" * 50 + "]" * 50,
    ]
    pieces = ["[", "]", "{", "}", ",", ":", '"a"', '"', "1", "-", ".", "e", "0", " ",
              "null", "tru", "NaN", "\\", "\\u00", "\n"]
    random_docs = ["".join(rng.choice(pieces) for _ in range(rng.randint(1, 7)))
                   for _ in range(3000)]
    return hand + random_docs


def json_values():
    return [
        None, True, 1, -5, 2.5, float("nan"), float("inf"), float("-inf"), "s", "é\U0001f600",
        [], {}, [1, [2, []], {}], {"a": {"b": [1.0, "x"]}, "c": None},
        {"version": "0.2.1", "metadata": {"filament used [mm]": 4427.38, "x per tool": [1, 2]},
         "preview": {"resolution": "640x480", "data": "QUJD", "format": "PNG"}},
    ]


def sums(rng):
    hand = [[], [1.0], [0.1, 0.2, 0.3], [1e100, 1.0, -1e100], [-0.0], [-0.0, -0.0],
            [3067.34, 1274.58, 0.0, 0.0, 0.0], [9.37, 3.80, 0.00, 0.00, 0.00],
            [0.26, 0.10, 0.00, 0.00, 0.00], [float("inf"), 1.0], [float("inf"), float("-inf")],
            [1e308, 1e308, -1e308], [1, 2.5], [1, 0.1, 0.2], [True, 0.5]]
    random_sums = [[rng.uniform(-1e3, 1e3) for _ in range(rng.randint(1, 8))]
                   for _ in range(1000)]
    return hand + random_sums


def record():
    rng = random.Random(20261120)
    cases = {"python": sys.version.split()[0]}
    cases["int"] = [[s, outcome(lambda s=s: str(int(s)))] for s in number_strings(rng)]
    cases["float"] = [[s, outcome(lambda s=s: repr(float(s)))] for s in number_strings(rng)]
    cases["float_repr"] = [[x.hex(), repr(x)] for x in floats(rng)]
    cases["round2"] = [[x.hex(), repr(round(x, 2))] for x in floats(rng)]
    cases["int_of_float"] = [[x.hex(), outcome(lambda x=x: str(int(x)))] for x in floats(rng)]
    cases["repr_str"] = [[s, repr(s)] for s in strings(rng) if no_surrogates(s)]
    loads = []
    for doc in json_docs(rng):
        result = outcome(lambda doc=doc: json.dumps(json.loads(doc)))
        if no_surrogates(doc) and (result[0] != "ok" or no_surrogates(json.loads(doc).__repr__())):
            loads.append([doc, result])
    cases["json_loads"] = loads
    cases["json_dump_indent2"] = [[json.dumps(v), json.dumps(v, indent=2)] for v in json_values()]
    cases["sum"] = [[[x.hex() if isinstance(x, float) else x for x in values],
                     outcome(lambda values=values: repr(sum(values)))] for values in sums(rng)]
    return json.dumps(cases, indent=1, ensure_ascii=True) + "\n"


def main():
    text = record()
    if "--check" in sys.argv[1:]:
        if OUT.read_text(encoding="utf-8") != text:
            print(f"{OUT} differs from what CPython {sys.version.split()[0]} records")
            return 1
        print(f"{OUT.name}: identical to CPython {sys.version.split()[0]}")
        return 0
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
