from prusa.link import util

# Each case: the tty devices pyudev lists, each its properties (a missing key: not there); the
# printers PrusaLink's get_usb_printers gives, each [vendor_id, model_id, serial_number, path].
request = json.load(sys.stdin)


class Device:
    def __init__(self, properties):
        self.properties = properties


class Context:
    devices = []

    def list_devices(self, subsystem):
        assert subsystem == 'tty'
        return [Device(p) for p in Context.devices]


util.pyudev.Context = Context
results = []
for devices in request["cases"]:
    Context.devices = devices
    results.append([[d.vendor_id, d.model_id, d.serial_number, d.path]
                    for d in util.get_usb_printers()])
print(json.dumps({"results": results}))
