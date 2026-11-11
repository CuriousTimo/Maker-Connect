#!/usr/bin/env python3
"""DYMO LabelManager: our agent against labelle, mechanically, operation for operation.

The one reference is labelle (labelle-org/labelle, Apache-2.0) at LABELLE_COMMIT, checked out
in reference/labelle and installed (with its own dependencies: real pyusb, real Pillow) in
reference/.venv-audit. Run with any Python; the script re-runs itself in that venv.

1. labelle's real code prints each example as its CLI does (`cli.py:586`, `590`, `613`):
   `UsbDevice(dev)`, `device.setup()`, `DymoLabeler(tape_size_mm, device)` +
   `DymoLabeler.print(image)` for pictures, or `DymoLabelerFunctions.print_label(lines)` +
   `dispose()` (what `print` does after turning its image into lines) for line examples.
   The only thing not real is the USB device: a fake pyusb backend with the PnP's own
   descriptors (read on the agent's Orange Pi, 0922:1001) records every operation pyusb
   asks of it (configuration, kernel driver, claim, transfers with their timeouts, release).
2. Our test (`crates/label/tests/labelle.rs`) does the same examples with our
   `dymo::session` on a recorder with the same descriptors.
3. Every operation of both is compared; the result goes into docs/audits/dymo-labelmanager.md
   between the markers. Exit 0 only when every example is identical.
"""

from __future__ import annotations

import array
import hashlib
import os
import subprocess
import sys
from pathlib import Path

LABELLE_COMMIT = "5e8f1e5d4f2f4447c8827df96c174812275d678f"
ROOT = Path(__file__).resolve().parent.parent
LABELLE = ROOT / "reference" / "labelle"
VENV_PY = ROOT / "reference" / ".venv-audit" / "bin" / "python"
OUT = ROOT / "crates" / "label" / "tests" / "labelle"
DOC = ROOT / "docs" / "audits" / "dymo-labelmanager.md"
BEGIN, END = "<!-- generated: begin -->", "<!-- generated: end -->"

# The PnP's descriptors, as read from sysfs on the agent's Orange Pi (2026-11-10): one
# configuration (active: 1); interface 0 HID (usbhid bound), interrupt OUT 0x01 / IN 0x81;
# interface 1 mass storage (usb-storage bound), bulk 0x02 / 0x82; 64-byte packets.
DEVICE = {"vendor": 0x0922, "product": 0x1001, "active": 1,
          "interfaces": [(0, 0, 0x03, [(0x01, 3), (0x81, 3)], True),
                         (1, 0, 0x08, [(0x02, 2), (0x82, 2)], True)]}
# What the fake device answers a read (labelle throws answers away; any answer does).
ANSWER = [0x40, 0x00, 0x12, 0x4E, 0x0D, 0x6B, 0x00, 0x00]
# Our 16 mm of blank tape after a label (dymo::TRAILING_MM at 180 dpi): part of the picture,
# so labelle's picture gets the same blank columns at its end.
TRAILING_COLUMNS = 113


def pattern(lines: int, width: int, seed: int) -> list[list[int]]:
    """Example lines: varied bytes, 0x16 (SYN) among them now and then."""
    return [[(i * 31 + j * 17 + seed) % 256 for j in range(width)] for i in range(lines)]


def picture(width: int, height: int, seed: int) -> list[list[int]]:
    """An example picture, rows of 0/1 (1 burned): edges, a diagonal and a varied fill."""
    return [[1 if (x == 0 or y == 0 or x == width - 1 or y == height - 1 or x == y
                   or (x * 7 + y * 3 + seed) % 11 == 0) else 0
             for x in range(width)] for y in range(height)]


# kind, name, tape mm, data, what it shows
CASES = [
    ("lines", "short", 12, pattern(10, 8, 5), "10 lines, 12 mm (8 bytes a line)"),
    ("lines", "blank", 12, [[0] * 8 for _ in range(114)], "114 blank lines"),
    ("lines", "one_transfer_edge", 12, pattern(63, 8, 1), "63 lines: just under one transfer"),
    ("lines", "two_transfers", 12, pattern(64, 8, 2), "64 lines: the 64th SYN starts a second transfer"),
    ("lines", "group_edge", 12, pattern(201, 8, 3), "201 lines: still one group (labelle: while > 201)"),
    ("lines", "two_groups", 12, pattern(202, 8, 4), "202 lines: 200 + 2, ESC D only once"),
    ("lines", "long", 12, pattern(650, 8, 6), "650 lines: groups 200, 200, 200, 50"),
    ("lines", "syn_in_data", 12, [[0x16] * 8 for _ in range(20)], "every byte 0x16: data counts as SYN"),
    ("lines", "narrow_6mm", 6, pattern(70, 4, 7), "6 mm tape (4 bytes a line)"),
    ("lines", "tape_9mm", 9, pattern(70, 6, 8), "9 mm tape (6 bytes a line)"),
    ("lines", "tape_19mm", 19, pattern(70, 12, 9), "19 mm tape (12 bytes a line)"),
    ("picture", "picture_12mm", 12, picture(150, 64, 1), "a picture 150 × 64, 12 mm: image to bytes"),
    ("picture", "picture_6mm", 6, picture(90, 32, 2), "a picture 90 × 32, 6 mm"),
    ("picture", "picture_9mm", 9, picture(90, 48, 3), "a picture 90 × 48, 9 mm"),
    ("picture", "picture_long", 12, picture(400, 64, 4), "a picture 400 × 64: two groups"),
]


def check_labelle() -> None:
    """The labelle that runs is reference/labelle at LABELLE_COMMIT, unchanged."""
    head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=LABELLE, capture_output=True,
                          text=True).stdout.strip()
    dirty = subprocess.run(["git", "status", "--porcelain", "src"], cwd=LABELLE,
                           capture_output=True, text=True).stdout.strip()
    if head != LABELLE_COMMIT or dirty:
        sys.exit(f"reference/labelle must be clean at {LABELLE_COMMIT} (is {head}, {dirty or 'clean'})")
    import labelle
    installed = Path(labelle.__file__).parent
    for rel in ["lib/devices/dymo_labeler.py", "lib/devices/usb_device.py", "lib/constants.py"]:
        a = hashlib.sha256((installed / rel).read_bytes()).hexdigest()
        b = hashlib.sha256((LABELLE / "src" / "labelle" / rel).read_bytes()).hexdigest()
        if a != b:
            sys.exit(f"installed labelle's {rel} differs from reference/labelle: reinstall it")


class Desc:
    """A descriptor: the given fields, 0 for any other pyusb reads."""
    def __init__(self, **fields): self.__dict__.update(fields)
    def __getattr__(self, name): return 0


class FakeBackend:
    """pyusb's IBackend for one PnP: every operation besides reading descriptors recorded."""

    def __init__(self, log: list[str]):
        self.log = log
        self.active = DEVICE["active"]
        self.drivers = {n: bound for n, _, _, _, bound in DEVICE["interfaces"]}

    # Descriptors: read from what the kernel keeps, nothing on the wire; not recorded.
    def enumerate_devices(self): yield "pnp"
    def get_device_descriptor(self, dev):
        return Desc(bLength=18, bDescriptorType=1, idVendor=DEVICE["vendor"],
                    idProduct=DEVICE["product"], bNumConfigurations=1, bus=3, address=65)
    def get_configuration_descriptor(self, dev, config):
        if config != 0: raise IndexError
        return Desc(bLength=9, bDescriptorType=2, bNumInterfaces=len(DEVICE["interfaces"]),
                    bConfigurationValue=1)
    def get_interface_descriptor(self, dev, intf, alt, config):
        for n, a, cls, eps, _ in DEVICE["interfaces"]:
            if n == intf and a == alt:
                return Desc(bLength=9, bDescriptorType=4, bInterfaceNumber=n,
                            bAlternateSetting=a, bInterfaceClass=cls, bNumEndpoints=len(eps))
        raise IndexError
    def get_endpoint_descriptor(self, dev, ep, intf, alt, config):
        for n, a, _, eps, _ in DEVICE["interfaces"]:
            if n == intf and a == alt:
                address, kind = eps[ep]
                return Desc(bLength=7, bDescriptorType=5, bEndpointAddress=address,
                            bmAttributes=kind, wMaxPacketSize=64)
        raise IndexError

    # Everything else: recorded.
    def open_device(self, dev): self.log.append("open_device"); return "handle"
    def close_device(self, h): self.log.append("close_device")
    def get_configuration(self, h): self.log.append(f"get_configuration -> {self.active}"); return self.active
    def set_configuration(self, h, v): self.log.append(f"set_configuration {v}"); self.active = v
    def is_kernel_driver_active(self, h, i):
        self.log.append(f"is_kernel_driver_active {i} -> {self.drivers[i]}"); return self.drivers[i]
    def detach_kernel_driver(self, h, i): self.log.append(f"detach_kernel_driver {i}"); self.drivers[i] = False
    def claim_interface(self, h, i): self.log.append(f"claim_interface {i}")
    def release_interface(self, h, i): self.log.append(f"release_interface {i}")
    def intr_write(self, h, ep, intf, data, timeout):
        self.log.append(f"intr_write {ep:02x} {bytes(data).hex()} {timeout}"); return len(data)
    def bulk_write(self, h, ep, intf, data, timeout):
        self.log.append(f"bulk_write {ep:02x} {bytes(data).hex()} {timeout}"); return len(data)
    def intr_read(self, h, ep, intf, buff, timeout):
        self.log.append(f"intr_read {ep:02x} {len(buff)} {timeout}")
        buff[:len(ANSWER)] = array.array("B", ANSWER); return len(ANSWER)
    def bulk_read(self, h, ep, intf, buff, timeout):
        self.log.append(f"bulk_read {ep:02x} {len(buff)} {timeout}")
        buff[:len(ANSWER)] = array.array("B", ANSWER); return len(ANSWER)
    def __getattr__(self, name):
        def other(*args):
            self.log.append(f"{name} (not expected)")
        return other


def run_labelle(kind: str, mm: int, data) -> list[str]:
    import usb.core
    from PIL import Image
    from labelle.lib.devices.dymo_labeler import DymoLabeler, DymoLabelerFunctions
    from labelle.lib.devices.usb_device import UsbDevice
    log: list[str] = []
    dev = usb.core.find(backend=FakeBackend(log), idVendor=DEVICE["vendor"])
    device = UsbDevice(dev)
    # As labelle's CLI (cli.py:586, 590): setup, then the labeler with the device.
    device.setup()
    labeler = DymoLabeler(tape_size_mm=mm, device=device)
    if kind == "picture":
        rows = [row + [0] * TRAILING_COLUMNS for row in data]
        image = Image.new("1", (len(rows[0]), len(rows)))
        image.putdata([p for row in rows for p in row])
        labeler.print(image)
    else:
        DymoLabelerFunctions(devout=device.devout, devin=device.devin, synwait=64) \
            .print_label([list(l) for l in data])
        device.dispose()
    return log


def read_cases(path: Path) -> dict[str, list[str]]:
    cases, name = {}, None
    for line in path.read_text().splitlines():
        if line.startswith("case "):
            name = line[5:]; cases[name] = []
        elif line and name:
            cases[name].append(line)
    return cases


def summary(ops: list[str]) -> str:
    writes = [o for o in ops if o.startswith(("intr_write", "bulk_write"))]
    reads = [o for o in ops if o.startswith(("intr_read", "bulk_read"))]
    status = sum(1 for o in writes if o.split()[2] == "1b41")
    setup = [o.split(" ->")[0] for o in ops if not o.startswith(("intr_", "bulk_"))]
    timeouts = sorted({o.split()[-1] for o in writes + reads})
    return (f"{', '.join(setup)}; {len(writes)} writes ({status} × `1B 41`), {len(reads)} reads "
            f"of {reads[0].split()[2] if reads else '–'} bytes; timeout {'/'.join(timeouts)} ms")


def main() -> int:
    if Path(sys.executable).resolve() != VENV_PY.resolve() and VENV_PY.exists():
        os.execv(str(VENV_PY), [str(VENV_PY), __file__])
    check_labelle()
    OUT.mkdir(parents=True, exist_ok=True)
    device_line = "device active {active} ".format(**DEVICE) + " ".join(
        f"iface {n} {a} {cls:02x} {'+' if bound else '-'} " + ",".join(f"{e:02x}{'i' if k == 3 else 'b'}" for e, k in eps)
        for n, a, cls, eps, bound in DEVICE["interfaces"])
    with open(OUT / "cases.txt", "w") as f:
        f.write(device_line + "\n")
        for kind, name, mm, data, _ in CASES:
            f.write(f"case {name}\n{kind} {mm}\n")
            for row in data:
                f.write((bytes(row).hex() if kind == "lines" else "".join(map(str, row))) + "\n")
    with open(OUT / "labelle.txt", "w") as f:
        for kind, name, mm, data, _ in CASES:
            f.write(f"case {name}\n" + "".join(o + "\n" for o in run_labelle(kind, mm, data)))
    (OUT / "ours.txt").unlink(missing_ok=True)
    test = subprocess.run(["cargo", "test", "-q", "-p", "maker-connect-label", "--test", "labelle"],
                          cwd=ROOT, capture_output=True, text=True)
    if not (OUT / "ours.txt").exists():
        print(test.stdout[-3000:], test.stderr[-3000:]); return 1
    theirs, ours = read_cases(OUT / "labelle.txt"), read_cases(OUT / "ours.txt")
    rows, bad = [], 0
    for _, name, _, _, what in CASES:
        a, b = theirs.get(name, []), ours.get(name, [])
        if a == b:
            result = "**identical**"
        else:
            bad += 1
            at = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
            x = a[at][:70] if at < len(a) else "(end)"
            y = b[at][:70] if at < len(b) else "(end)"
            result = f"**DIFFERS** at operation {at + 1}: labelle `{x}`, ours `{y}`"
        rows.append(f"| `{name}` | {what} | {summary(a)} | {summary(b)} | {result} |")
    import PIL, usb
    table = "\n".join([
        BEGIN,
        f"labelle `{LABELLE_COMMIT[:7]}` with pyusb {usb.__version__} and Pillow {PIL.__version__}, "
        "run by `scripts/dymo-audit-check.py`. Every operation of both, in order, is in "
        "`crates/label/tests/labelle/labelle.txt` and `ours.txt`.",
        "",
        "| Example | What it shows | labelle (its own code, run) | Ours (`dymo::session`) | Result |",
        "|---|---|---|---|---|",
        *rows,
        "",
        f"**{len(CASES) - bad} of {len(CASES)} identical.**",
        END,
    ])
    doc = DOC.read_text()
    DOC.write_text(doc[:doc.index(BEGIN)] + table + doc[doc.index(END) + len(END):])
    print(f"{len(CASES) - bad} of {len(CASES)} examples identical to labelle")
    return 0 if bad == 0 and test.returncode == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
