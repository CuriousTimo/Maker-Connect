from prusa.link.printer_adapter.model import Model
from prusa.link.printer_adapter.structures.model_classes import Telemetry
from prusa.link.printer_adapter.structures.module_data_classes import SerialAdapterData

# A new model: its telemetry, each of the ten fields read before it is set, one set and read.
FIELDS = ["serial_adapter", "file_printer", "print_stats", "state_manager", "job",
          "ip_updater", "sd_card", "folder_storage", "filesystem_storage", "mmu_observer"]


def read(model, name):
    try:
        getattr(model, name)
        return {"ok": True}
    except Exception as error:  # pylint: disable=broad-except
        return {"error": [type(error).__name__, str(error)]}


request = json.load(sys.stdin)
model = Model()
out = {
    "telemetry_is_default": model.latest_telemetry == Telemetry(),
    "unset": [read(model, name) for name in FIELDS],
}
model.serial_adapter = SerialAdapterData(using_port=None, reset_disabling=False)
out["set"] = read(model, "serial_adapter")
out["reset_disabling"] = model.serial_adapter.reset_disabling
data = model.serial_adapter  # serial_adapter.py:92, then changed through the data
data.reset_disabling = True
out["shared"] = model.serial_adapter.reset_disabling
out["others_still_unset"] = [read(model, name) for name in FIELDS[1:]]
print(json.dumps(out))
