# pyudev itself, not the stub: PrusaLink's calls (`util.py:256-257`), each device's properties as
# get_usb_printers reads them (a missing one None), in pyudev's order.
STUB.discard('pyudev')
import pyudev  # noqa: E402

request = json.load(sys.stdin)
devices = []
for device in pyudev.Context().list_devices(subsystem='tty'):
    devices.append([device.properties.get(key) for key in request["keys"]])
print(json.dumps({"devices": devices}))
