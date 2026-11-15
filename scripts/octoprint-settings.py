#!/usr/bin/env python3
"""Writes OctoPrint 1.11.8's default settings exactly as `settings/__init__.py:94` builds them
(`Config().model_dump(by_alias=True)`), as JSON (`mode="json"`, so str-Enum values are
written as their strings), for the Marlin port
(crates/engine/src/ports/octoprint/default_settings.json). Run from the repository root with the
venv that has OctoPrint's pydantic: reference/.venv-octoprint/bin/python."""
import json, sys

sys.path.insert(0, "reference/OctoPrint-1.11.8/src")
from octoprint.schema.config import Config  # noqa: E402

OUT = "crates/engine/src/ports/octoprint/default_settings.json"
data = Config().model_dump(by_alias=True, mode="json")
with open(OUT, "w") as f:
    json.dump(data, f, indent=1, sort_keys=False)
    f.write("\n")
print(f"{len(data)} sections written to {OUT}")
