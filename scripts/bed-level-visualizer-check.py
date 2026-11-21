#!/usr/bin/env python3
"""The BedLevelVisualizer port's mechanical check (docs/audits/bed-level-visualizer.md).

It runs OctoPrint-BedLevelVisualizer's own `process_gcode` (reference/OctoPrint-BedLevelVisualizer
at 05ba418, octoprint_bedlevelvisualizer/__init__.py, unchanged) on each case of
crates/hub/src/devices/bed_level_visualizer_cases.json, from `enable_mesh_collection` on, with
BLV's default settings, and compares the mesh it sends with
crates/hub/src/devices/bed_level_visualizer_expected.json. The Rust port is held to that same file
by the hub's test `devices::bed_level_visualizer::tests::the_reference_cases`.

Harness, all of it: OctoPrint's `octoprint.plugin`, `octoprint.events` and `flask` are empty
stand-ins; `_settings` answers BLV's own defaults (`get_settings_defaults`); the printer profile is
rectangular, without a custom box; plugin messages go nowhere; the mesh is the one
`send_mesh_data_collected_event` gets, the last of a case. An exception in `process_gcode` is
caught and the next line goes on, as OctoPrint's comm does with a hook's. Each point is printed as
`round(float(v), 4)`, or null where `float` fails (the page then shows no value).

`--write` rewrites the expected file and the generated block of the audit. Run with
`python3 -I scripts/bed-level-visualizer-check.py`. Standard library only."""
import json, os, sys, types

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REF = os.path.join(ROOT, "reference", "OctoPrint-BedLevelVisualizer")
CASES = os.path.join(ROOT, "crates", "hub", "src", "devices", "bed_level_visualizer_cases.json")
EXPECTED = os.path.join(ROOT, "crates", "hub", "src", "devices", "bed_level_visualizer_expected.json")
DOC = os.path.join(ROOT, "docs", "audits", "bed-level-visualizer.md")
BEGIN, END = "<!-- generated: begin -->", "<!-- generated: end -->"


def reference():
    for name in ["octoprint", "octoprint.plugin", "octoprint.events", "flask"]:
        sys.modules[name] = types.ModuleType(name)
    plugin = sys.modules["octoprint.plugin"]
    for n in ["StartupPlugin", "TemplatePlugin", "AssetPlugin", "SettingsPlugin", "WizardPlugin",
              "SimpleApiPlugin", "EventHandlerPlugin", "BlueprintPlugin"]:
        setattr(plugin, n, type(n, (), {}))
    plugin.BlueprintPlugin.route = staticmethod(lambda *a, **k: (lambda f: f))
    sys.modules["octoprint"].plugin = plugin

    class Events:
        PRINT_STARTED = "PrintStarted"
        PRINT_DONE = "PrintDone"
        PLUGIN_BEDLEVELVISUALIZER_MESH_DATA_COLLECTED = "mesh_data_collected"

    sys.modules["octoprint.events"].Events = Events
    sys.path.insert(0, REF)
    from octoprint_bedlevelvisualizer import bedlevelvisualizer
    return bedlevelvisualizer


class Settings:
    def __init__(self, d):
        self.d = d

    def get_boolean(self, k):
        return bool(self.d[k[0]])

    def get(self, k):
        return self.d[k[0]]

    def get_int(self, k):
        return int(self.d[k[0]])


class Nowhere:
    def __getattr__(self, name):
        return lambda *a, **k: None


def point(v):
    try:
        return round(float(v), 4)
    except (TypeError, ValueError):
        return None


def run(cls, lines):
    b = cls()
    b._settings = Settings(b.get_settings_defaults())
    b._plugin_manager = Nowhere()
    b._event_bus = Nowhere()
    b._identifier = "bedlevelvisualizer"
    b._printer_profile_manager = types.SimpleNamespace(get_current=lambda: {"volume": {
        "formFactor": "rectangular", "custom_box": False, "width": 200, "depth": 200, "height": 200}})
    sent = []
    b.send_mesh_data_collected_event = lambda mesh, bed: sent.append(mesh)
    b.enable_mesh_collection()
    for line in lines:
        try:
            b.process_gcode(None, line)
        except Exception:
            pass
    return None if not sent else [[point(v) for v in row] for row in sent[-1]]


def main():
    cls = reference()
    cases = json.load(open(CASES))
    got = [run(cls, case["lines"]) for case in cases]
    rows = ["| Case | What | BLV's mesh |", "|---|---|---|"]
    for i, (case, mesh) in enumerate(zip(cases, got)):
        rows.append(f"| {i} | {case['what']} | {json.dumps(mesh) if mesh else 'none'} |")
    block = "\n".join(rows)
    if "--write" in sys.argv:
        json.dump(got, open(EXPECTED, "w"))
        open(EXPECTED, "a").write("\n")
        doc = open(DOC).read()
        head, rest = doc.split(BEGIN)
        _, tail = rest.split(END)
        open(DOC, "w").write(f"{head}{BEGIN}\n{block}\n{END}{tail}")
    expected = json.load(open(EXPECTED))
    if got != expected:
        for i, (a, b) in enumerate(zip(got, expected)):
            if a != b:
                print(f"UNEXPECTED case {i}: BLV {a}, expected file {b}")
        sys.exit(1)
    doc = open(DOC).read()
    if block not in doc:
        print("docs/audits/bed-level-visualizer.md: the generated block is stale (run with --write)")
        sys.exit(1)
    print(f"ok: {len(cases)} cases, BLV's meshes = the expected file")


main()
