#!/usr/bin/env python3
"""Runs the REAL OctoPrint 1.11.8 `Settings.loadScript` / `listScripts` and writes the outputs to
crates/engine/src/ports/octoprint/scripts_expected.json. The Rust test `scripts_identical_to_octoprint`
(ports/octoprint/settings.rs) renders the same cases with our port and compares byte for byte.

Run with the reference venv:
    reference/.venv-octoprint/bin/python scripts/octoprint-scripts-check.py

Needs OctoPrint's dependencies (setup.py `core_deps`) in that venv (jinja2 comes with flask>=3.0.3).
"""
import copy
import itertools
import json
import os
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "reference", "OctoPrint-1.11.8", "src"))

import jinja2  # noqa: E402
import octoprint.settings as ops  # noqa: E402

OUT = os.path.join(ROOT, "crates/engine/src/ports/octoprint/scripts_expected.json")

# Extra scripts, added to the real settings as an overlay (and in ours by the test) to exercise the templating
# beyond the defaults. Keys are paths below `scripts.gcode`.
EXTRA = {
    "t_plugins": "{{ plugins.foo.bar }}|{{ script.type }}:{{ script.name }}\n",
    "t_trailing": "a\n\n",
    "t_trailing_one": "a\n",
    "t_blocks": "{% if x %}\nA\n{% endif %}\nB\n{% for i in range(2) %}\nL{{ i }}\n{% endfor %}\nZ\n",
    "t_snip_abs": "{% snippet '/gcode/snippets/disable_bed' %}",
    "t_snip_ws": "x\n  {%- snippet 'disable_bed' -%}  \ny",
    "t_snip_dq": '{% snippet "disable_bed" %}|',
    "t_snip_nonliteral": "{% snippet foo %}",
    "t_include_rel": "{% include 'snippets/disable_bed' %}",
    "t_include_abs": "{% include '/gcode/snippets/disable_bed' %}",
    "t_include_bundled": "{% include 'bundled:/gcode/afterPrintCancelled' %}",
    "t_include_file": "{% include 'file:x' %}",
    "t_include_missing": "{% include 'snippets/zzz' %}",
    "t_include_ignore": "{% include 'zzz' ignore missing %}end",
    "t_undef": "[{{ nothing }}]",
    "t_undef_attr": "[{{ a.b.c }}]",
    "t_syntax_error": "{% if %}",
    "t_loop": "{% for t in range(printer_profile.extruder.count) %}{{ loop.index }}{{ loop.last }}{{ t }},{% endfor %}",
    "t_arith": "{{ 7 // 2 }} {{ 2 ** 3 }} {{ 'a'|upper }} {{ [1, 2]|join(',') }} {{ 'x' ~ 1 }}",
    "t_bool": "{{ printer_profile.heatedBed }}",
    "t_empty": "",
    "t_floats": "{{ last_position.x }} {{ last_position.f }} {{ pause_position.z }} {{ pause_position.y }} "
                "{{ 1/2 }} {{ 0.1 + 0.2 }} {{ 3.0 }} {{ -0.0 }} {{ last_position.x * 2 }} {{ last_position.x|int }}",
    # known to differ (KNOWN_DIFFER in the Rust test) -- except t_int_key*, which differ only when the
    # context has no int keys (see INT_KEY_TAG):
    "t_float_exp": "{{ 1e20 }} {{ pause_position.x }}",
    "t_round": "{{ last_position.e|round(1) }}",
    "t_int_key": "{{ last_temperature[0].actual }}",
    "t_int_key_more": "{{ last_temperature[1] is defined }}|{{ last_temperature['b'].actual }}"
                      "|{{ last_temperature['0'] is defined }}|{{ last_temperature|length }}",
    "t_dict_order": "{% for k in last_temperature %}{{ k }}:{{ last_temperature[k].actual is none }};{% endfor %}"
                    "{% for k in printer_profile.extruder %}{{ k }},{% endfor %}",
    "t_none": "[{{ last_position.f }}]{% if last_position.f is none %}N{% endif %}{% if not pause_position.z %}Z{% endif %}",
    "t_raw_snip": "{% raw %}{% snippet 'disable_bed' %}{% endraw %}",
    "t_comment_snip": "{# {% snippet 'disable_bed' %} #}x",
    "nested": {"inner": "N{{ 1 + 2 }}"},
}

CASE_NAMES_MISSING = [
    "doesNotExist", "snippets/doesNotExist", "", "snippets", "nested", "bundled:afterPrintCancelled",
    "bundled:snippets/disable_bed", "file:afterPrintCancelled", "unknown:afterPrintCancelled",
    "afterPrintCancelled/x",
]


# JSON cannot carry int keys (json.dump writes `0` as "0"), but `as_script_dict` (comm.py:407) has them
# (`last_temperature[0]`). A context key that is an int is written as the string "#int:<n>"; the Rust
# test (settings.rs, `tagged_context`) turns it back into an int key.
INT_KEY_TAG = "#int:"


def tag_int_keys(value):
    if isinstance(value, dict):
        return {(f"{INT_KEY_TAG}{k}" if isinstance(k, int) and not isinstance(k, bool) else k): tag_int_keys(v)
                for k, v in value.items()}
    if isinstance(value, list):
        return [tag_int_keys(v) for v in value]
    return value


def contexts():
    out = [None, {}]
    for count, shared, bed in itertools.product(range(0, 5), (True, False), (True, False)):
        out.append({"printer_profile": {"extruder": {"count": count, "sharedNozzle": shared}, "heatedBed": bed}})
    base = {"printer_profile": {"extruder": {"count": 2, "sharedNozzle": False}, "heatedBed": True}}
    # as comm.py:1391 does: dict_merge(context, {"plugins": {name: variables}})
    out.append({**base, "plugins": {"foo": {"bar": "baz"}}})
    out.append({**base, "plugins": {"foo": {"bar": 1}, "other": {"x": [1, 2]}}})
    out.append({"x": True, **base})
    # float and None values, shaped as comm.py:1322-1393 passes last_position / last_temperature / pause_*
    out.append({**base,
                "last_position": {"x": 10.5, "y": 0.1, "z": 2.0, "e": 1.25, "f": None, "t": 0},
                "last_temperature": {0: {"actual": 210.3, "target": 215.0}, "b": {"actual": 59.9, "target": None}},
                "pause_position": {"x": 1e-7, "y": 123456789.123, "z": None}})
    # as `as_script_dict` builds it: tools (int), "b", "c", then custom identifiers, in insertion order
    out.append({**base,
                "last_temperature": {0: {"actual": 21.5, "target": 0.0}, 1: {"actual": 30.0, "target": 210.0},
                                     "b": {"actual": 22.0, "target": 60.0}, "c": {"actual": None, "target": None},
                                     "zz": {"actual": 1.0, "target": None}, "aa": {"actual": 2.0, "target": None}}})
    out.append({**base, "last_temperature": {"b": {"actual": 22.0, "target": 60.0}, 1: {"actual": 30.0, "target": 1.0},
                                              0: {"actual": 21.5, "target": 0.0}}})
    return out


def main():
    basedir = tempfile.mkdtemp(prefix="octoprint-scripts-")
    s = ops.settings(init=True, basedir=basedir)
    # an overlay is how plugins add to the settings (`add_overlay`, `:1157`); `set` ignores paths
    # that are not in the defaults
    s.add_overlay({"scripts": {"gcode": copy.deepcopy(EXTRA)}})

    names = s.listScripts("gcode")
    all_names = names + CASE_NAMES_MISSING
    cases = []
    for name in all_names:
        for ctx in contexts():
            given = tag_int_keys(copy.deepcopy(ctx))
            result = s.loadScript("gcode", name, context=copy.deepcopy(ctx))
            cases.append({"type": "gcode", "name": name, "context": given, "expected": result})
    # another script type that does not exist
    cases.append({"type": "nope", "name": "afterPrintCancelled", "context": None,
                  "expected": s.loadScript("nope", "afterPrintCancelled")})

    data = {
        "source": "OctoPrint 1.11.8 settings/__init__.py Settings.loadScript / listScripts",
        "jinja2": jinja2.__version__,
        "python": sys.version.split()[0],
        "extra_scripts": EXTRA,
        "list_scripts": {"gcode": names, "nope": s.listScripts("nope")},
        "cases": cases,
    }
    with open(OUT, "w") as f:
        json.dump(data, f, indent=1, ensure_ascii=False)
        f.write("\n")
    none = sum(1 for c in cases if c["expected"] is None)
    print(f"{len(cases)} cases ({none} give None), jinja2 {jinja2.__version__}; wrote {OUT}")


if __name__ == "__main__":
    main()
