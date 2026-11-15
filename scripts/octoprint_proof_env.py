"""The environment both sides of the Marlin proof (docs/audits/octoprint-marlin.md) share: OctoPrint
1.11.8's own settings, plugin manager and its own virtual printer plugin
(`plugins/virtual_printer`, UNCHANGED), set up the way `octoprint/__init__.py` and `server/__init__.py`
set them up. Used by

* `scripts/octoprint-port-check.py` (the reference run: OctoPrint's `Printer` and `MachineCom`), and
* `scripts/octoprint-vp-bridge.py` (the bridge our Rust port talks to).

Both ask the plugin's own factory (`VirtualPrinterPlugin.virtual_printer_factory`) for the
`VirtualPrinter`, so it is the same Python class with the same settings on both sides.

Run only with `reference/.venv-octoprint-313/bin/python`.
"""

import json
import os
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OCTOPRINT_SRC = os.path.join(ROOT, "reference", "OctoPrint-1.11.8", "src")
sys.path.insert(0, OCTOPRINT_SRC)

# The two plugins of the Rust port's top edge: the test device and the prompt plugin
# (docs/agent-engine.md, step 3a); every other bundled plugin is disabled.
ENABLED_PLUGINS = ("virtual_printer", "action_command_prompt")

def merge(base, extra):
    out = dict(base)
    for key, value in extra.items():
        if isinstance(value, dict) and isinstance(out.get(key), dict):
            out[key] = merge(out[key], value)
        else:
            out[key] = value
    return out


def setup(config, workdir=None):
    """Settings, plugin manager and the plugin implementations, as the server sets them up.
    `config` is {"virtual_printer": {...}, "octoprint": {...}, "sd_files": {name: text}}: the settings
    changed from OctoPrint's defaults (listed with their reasons at the top of scenarios.json) and the
    virtual SD card's files.
    Returns (settings, plugin_manager, basedir)."""
    import yaml

    import octoprint.settings as ops

    basedir = workdir or tempfile.mkdtemp(prefix="octoprint-proof-")
    plugins_dir = os.path.join(OCTOPRINT_SRC, "octoprint", "plugins")
    all_plugins = sorted(
        d for d in os.listdir(plugins_dir) if os.path.isdir(os.path.join(plugins_dir, d)) and d != "__pycache__"
    )
    disabled = [p for p in all_plugins if p not in ENABLED_PLUGINS]

    vp = config.get("virtual_printer", {})
    octo = config.get("octoprint", {})
    yml = merge(octo, {"plugins": {"_disabled": disabled, "virtual_printer": vp}})
    # nothing the real OctoPrint would read from a profile folder or a user's login
    with open(os.path.join(basedir, "config.yaml"), "w") as f:
        yaml.safe_dump(yml, f)

    settings = ops.settings(init=True, basedir=basedir)

    # the virtual SD card's files (the plugin's own folder `virtualSd`)
    sd = settings.getBaseFolder("virtualSd")
    for name, text in config.get("sd_files", {}).items():
        with open(os.path.join(sd, name), "w") as f:
            f.write(text)

    from octoprint import init_pluginsystem

    pm = init_pluginsystem(settings, safe_mode=False)
    return settings, pm, basedir


def finish_plugins(settings, pm, components):
    """`server/__init__.py:_setup_plugin_manager`: inject the components and initialise the
    implementations."""
    from octoprint import octoprint_plugin_inject_factory as opif
    from octoprint import settings_plugin_inject_factory as spif

    pm.implementation_inject_factories = [opif(settings, components), spif(settings)]
    pm.initialize_implementations()


def make_virtual_printer(pm, read_timeout=5.0, baudrate=115200):
    """What `comm.py:_open_serial` does with the plugin's hook: the plugin's own factory."""
    hooks = pm.get_hooks("octoprint.comm.transport.serial.factory")
    for name, hook in hooks.items():
        obj = hook(None, "VIRTUAL", baudrate, read_timeout)
        if obj is not None:
            return obj
    raise RuntimeError("the virtual printer plugin gave no printer")


def lat1(data):
    """bytes as text, one character a byte (what the fixtures hold)"""
    return data.decode("latin-1")


def scenario_config(top, scn):
    """the settings of one scenario: the common ones, then the scenario's own, over the defaults"""
    return {
        "virtual_printer": merge(top.get("virtual_printer", {}), scn.get("virtual_printer", {})),
        "octoprint": merge(top.get("octoprint", {}), scn.get("octoprint", {})),
        "sd_files": scn.get("sd_files", {}),
    }


def load_config(path):
    with open(path) as f:
        return json.load(f)
