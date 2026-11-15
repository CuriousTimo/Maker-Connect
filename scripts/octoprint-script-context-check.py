#!/usr/bin/env python3
"""Prints what the REAL OctoPrint 1.11.8 renders for a gcode script that reads `last_temperature` the way
`MachineCom._getGcodeScript` (comm.py:1322) hands it over: `TemperatureRecord.as_script_dict()` (comm.py:407),
with int tool keys. The expected value of the Rust test `ours_get_gcode_script_int_keys` (ports/octoprint/comm.rs).

    reference/.venv-octoprint/bin/python scripts/octoprint-script-context-check.py
"""
import json
import os
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "reference", "OctoPrint-1.11.8", "src"))

import octoprint.settings as ops  # noqa: E402
from octoprint.util.comm import TemperatureRecord  # noqa: E402

SCRIPT = (
    "{{ last_temperature[0].actual }} {{ last_temperature[1].target }} {{ last_temperature.b.actual }}\n"
    "{% for k in last_temperature %}{{ k }},{% endfor %}"
)

s = ops.settings(init=True, basedir=tempfile.mkdtemp(prefix="octoprint-script-context-"))
s.add_overlay({"scripts": {"gcode": {"t_int_keys": SCRIPT}}})

t = TemperatureRecord()
t.set_tool(0, actual=21.5, target=0.0)
t.set_tool(1, actual=30.0, target=210.0)
t.set_bed(actual=22.0, target=60.0)
t.set_custom("zz", actual=1.0)
t.set_custom("aa", actual=2.0)

context = {"last_temperature": t.as_script_dict()}
print(json.dumps(s.loadScript("gcode", "t_int_keys", context=context)))
