import os
import time
from types import SimpleNamespace as NS

from prusa.link import util
from prusa.link.printer_adapter.structures.model_classes import (
    IndividualSlot, Slot, Telemetry)

# Each case: a function of `util.py` and its arguments; the result, or the exception's class and
# text. Bytes come as lists of numbers, a model's result as its `dict()`.
request = json.load(sys.stdin)


def plain(value):
    if isinstance(value, tuple):
        return [plain(item) for item in value]
    if isinstance(value, list):
        return [plain(item) for item in value]
    if isinstance(value, dict):
        return [[key, plain(item)] for key, item in value.items()]
    if isinstance(value, IndividualSlot):
        return value.dict()
    return value


def call(name, args):
    if name == "decode_line":
        return util.decode_line(bytes(args[0]))
    if name == "walk_dict":
        return list(util.walk_dict(args[0]))
    if name == "slots_with_param":
        fields, key, default, value = args
        slot = None if fields is None else Slot(active=fields.get("active"))
        model = NS(latest_telemetry=Telemetry(slot=slot))
        return util.slots_with_param(model, key, default, value)
    if name == "parse_little_endian_uint32":
        return util._parse_little_endian_uint32(NS(group=lambda _: args[0]))
    if name == "power_panic_delay":
        start = time.monotonic()
        result = util.power_panic_delay(NS(daemon=NS(power_panic_file=args[0])))
        return [result, time.monotonic() - start < 1]
    if name == "ensure_directory":
        path = args[0].replace("{side}", "python")
        return [util.ensure_directory(path), os.path.isdir(path)]
    return getattr(util, name)(*args)


results = []
for name, args in request["cases"]:
    try:
        results.append({"ok": plain(call(name, args))})
    except Exception as error:  # pylint: disable=broad-except
        results.append({"error": [type(error).__name__, str(error)]})
print(json.dumps({"results": results, "potato": util.is_potato_cpu()}))
