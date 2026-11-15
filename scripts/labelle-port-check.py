#!/usr/bin/env python3
"""DYMO LabelManager PnP port: our Rust port against labelle, mechanically.

The one reference is labelle (labelle-org/labelle, Apache-2.0) at LABELLE_COMMIT, checked out in
reference/labelle and installed (with its own runtime: real pyusb 1.3.1, real Pillow 12.3.0) in
reference/.venv-audit. Run with any Python; the script re-runs itself in that venv.

1. Real labelle's own command line (`labelle --picture FILE --tape-size-mm N`, typer's
   CliRunner on `labelle.cli.cli.app`) prints each example PNG on a fake pyusb backend: the
   devices of a scene with their descriptors (the PnP's own, read on the agent's Orange Pi,
   0922:1001), recording every operation pyusb asks of the backend (opening, string reads,
   configuration, kernel driver, claim, transfers with their timeouts, release, close).
2. Our test (`crates/engine/src/ports/labelle/tests.rs`) runs the same examples through the
   Rust port (`cli::print_picture`) on a recorder over the same scenes, writes `ours.txt`.
3. Every operation of both, and the final result line, is compared. The constants and command
   bytes are extracted from labelle's source (ast) and compared with the Rust constants. The
   audit rows go into docs/audits/dymo-labelmanager.md between the markers.
Exit 0 only when every example is identical.
"""

from __future__ import annotations

import array
import ast
import gc
import hashlib
import io
import os
import random
import struct
import subprocess
import sys
import zlib
from pathlib import Path

LABELLE_COMMIT = "5e8f1e5d4f2f4447c8827df96c174812275d678f"
ROOT = Path(__file__).resolve().parent.parent
LABELLE = ROOT / "reference" / "labelle"
VENV_PY = ROOT / "reference" / ".venv-audit" / "bin" / "python"
OUT = Path(os.environ.get("LABELLE_FIXTURES", ROOT / "crates" / "engine" / "tests" / "labelle"))
DOC = ROOT / "docs" / "audits" / "dymo-labelmanager.md"
BEGIN, END = "<!-- generated: begin -->", "<!-- generated: end -->"
PORTED = ["lib/devices/dymo_labeler.py", "lib/devices/usb_device.py", "lib/devices/device_manager.py",
          "lib/constants.py", "lib/utils.py", "lib/render_engines/picture.py",
          "lib/render_engines/margins.py", "lib/render_engines/print_payload.py",
          "lib/render_engines/horizontally_combined.py", "lib/render_engines/render_engine.py",
          "lib/render_engines/render_context.py", "cli/cli.py"]

# ---------------------------------------------------------------------------------------
# The scenes: the USB devices the fake backend shows. One text format both sides read.
# dev: bus, address, vendor, product, (iManufacturer, iProduct, iSerial), active, flags,
# interfaces [(number, alt, class, kernel driver bound, [(endpoint address, kind)])].
# ---------------------------------------------------------------------------------------
HID_STORAGE = [(0, 0, 0x03, True, [(0x01, 3), (0x81, 3)]), (1, 0, 0x08, True, [(0x02, 2), (0x82, 2)])]


def dev(bus, addr, vid, pid, strings=(0, 0, 0), active=1, flags=(), ifaces=()):
    return dict(bus=bus, addr=addr, vid=vid, pid=pid, strings=strings, active=active,
                flags=set(flags), ifaces=list(ifaces))


HUB = dev(1, 1, 0x1D6B, 0x0002, ifaces=[(0, 0, 0x09, False, [(0x81, 3)])])
LABELWRITER = dev(1, 4, 0x0922, 0x0020, strings=(1, 2, 3), ifaces=[(0, 0, 0x07, True, [(0x01, 2), (0x82, 2)])])


def pnp(bus=3, addr=65, pid=0x1001, **kw):
    kw.setdefault("strings", (1, 2, 3))
    kw.setdefault("ifaces", HID_STORAGE)
    return dev(bus, addr, 0x0922, pid, **kw)


SCENES = {
    # The system as the agent has it: a hub, a LabelWriter (labelle's vendor, not a LabelManager),
    # the PnP, and a second PnP that sorts after it.
    "normal": [HUB, LABELWRITER, pnp(), pnp(4, 9, 0x1002)],
    "single": [pnp()],
    "unconfigured": [pnp(active=0)],
    "printer_class": [pnp(ifaces=[(0, 0, 0x07, False, [(0x01, 2), (0x81, 2)])])],
    "no_interface": [pnp(ifaces=[(0, 0, 0x08, True, [(0x02, 2), (0x82, 2)])])],
    "no_endpoints": [pnp(ifaces=[(0, 0, 0x03, True, [(0x81, 3)])])],
    "no_dymo": [HUB],
    "only_labelwriter": [HUB, LABELWRITER],
    "no_langid": [pnp(flags=["nolangid"])],
    "write_timeout": [pnp(flags=["failwrite=3"])],
    "read_timeout": [pnp(flags=["failread=2"])],
    "busy_setconfig": [pnp(active=0, flags=["busy_setconfig"])],
    "denied_setconfig": [pnp(active=0, flags=["denied_setconfig"])],
    "no_kernel_api": [pnp(flags=["nokernelapi"])],
    "no_strings": [pnp(strings=(0, 0, 0))],
}

# ---------------------------------------------------------------------------------------
# The example pictures
# ---------------------------------------------------------------------------------------


def png_chunk(kind: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


def write_png(path: Path, width, height, depth, color, rows, plte=None, trns=None):
    """A PNG written by hand: filter 0, not interlaced, the rows as packed bytes."""
    raw = b"".join(b"\x00" + bytes(r) for r in rows)
    data = b"\x89PNG\r\n\x1a\n" + png_chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, depth, color, 0, 0, 0))
    if plte:
        data += png_chunk(b"PLTE", bytes(plte))
    if trns:
        data += png_chunk(b"tRNS", bytes(trns))
    data += png_chunk(b"IDAT", zlib.compress(raw)) + png_chunk(b"IEND", b"")
    path.write_bytes(data)


def pack_bits(values, depth):
    out, per = [], 8 // depth
    for i in range(0, len(values), per):
        byte = 0
        for j in range(per):
            v = values[i + j] if i + j < len(values) else 0
            byte |= v << (8 - depth * (j + 1))
        out.append(byte)
    return out


def picture(w, h, seed):
    """Edges, a diagonal, a disc and varied fill, as 0..255 grey."""
    rng = random.Random(seed)
    img = []
    for y in range(h):
        row = []
        for x in range(w):
            v = (x * 255 // max(1, w - 1) + rng.randrange(40)) % 256
            if x in (0, w - 1) or y in (0, h - 1) or x == y:
                v = 0
            if (x - w // 2) ** 2 + (y - h // 2) ** 2 < (h // 3) ** 2:
                v = 255 - v
            row.append(v)
        img.append(row)
    return img


def make_pngs(d: Path) -> dict[str, str]:
    """Writes the example PNGs; gives the name of each and what it is."""
    from PIL import Image
    d.mkdir(parents=True, exist_ok=True)
    what = {}

    def save(name, im, desc, **kw):
        im.save(d / f"{name}.png", **kw)
        what[name] = desc

    def grey(w, h, seed):
        return picture(w, h, seed)

    g = grey(150, 64, 1)
    rgb = Image.new("RGB", (150, 64))
    rgb.putdata([(v, (v * 3) % 256, 255 - v) for row in g for v in row])
    save("rgb", rgb, "RGB 150 x 64")
    rgba = Image.new("RGBA", (120, 64))
    g2 = grey(120, 64, 2)
    rgba.putdata([(v, 255 - v, (v * 5) % 256, (x * 255 // 119)) for y, row in enumerate(g2) for x, v in enumerate(row)])
    save("rgba", rgba, "RGBA 120 x 64, alpha varying (ignored by convert L)")
    gr = Image.new("L", (100, 64))
    gr.putdata([v for row in grey(100, 64, 3) for v in row])
    save("grey", gr, "grey 100 x 64 (dithered)")
    save("one_bit", gr.point(lambda v: 255 if v > 100 else 0).convert("1"), "1-bit 100 x 64")
    pal = gr.quantize(16)
    save("palette", pal, "palette 100 x 64, 16 colours")
    save("palette_trns", pal, "palette with a transparent entry", transparency=3)
    la = Image.new("LA", (100, 64))
    la.putdata([(v, (x * 255) // 99) for y, row in enumerate(grey(100, 64, 4)) for x, v in enumerate(row)])
    save("la", la, "grey and alpha 100 x 64")
    hub = Image.new("L", (400, 64), 255)
    hub.putdata([0 if (x // 8 + y // 8) % 2 == 0 and 10 < y < 54 else 255 for y in range(64) for x in range(400)])
    save("hub_label", hub, "as the hub's Label::to_png: grey 8-bit, 0 or 255, 400 x 64")
    wide = Image.new("L", (700, 64), 255)
    wide.putdata([0 if (x * 7 + y * 3) % 11 == 0 else 255 for y in range(64) for x in range(700)])
    save("wide", wide, "grey 700 x 64: many groups of lines")
    short = Image.new("L", (80, 40))
    short.putdata([v for row in grey(80, 40, 5) for v in row])
    save("short", short, "grey 80 x 40 on a 64-dot tape: fewer bytes a line")
    # Taller than the tape: labelle resizes it (bicubic; nearest for 1 and P).
    tall = grey(200, 160, 6)
    for name, mode in [("tall_rgb", "RGB"), ("tall_rgba", "RGBA"), ("tall_grey", "L"), ("tall_la", "LA")]:
        im = Image.new(mode, (200, 160))
        px = []
        for y, row in enumerate(tall):
            for x, v in enumerate(row):
                px.append({"RGB": (v, (v * 3) % 256, 255 - v), "RGBA": (v, (v * 3) % 256, 255 - v, 40 + (x % 200)),
                           "L": v, "LA": (v, 30 + x % 220)}[mode])
        im.putdata(px)
        save(name, im, f"{mode} 200 x 160: taller than the tape, resized")
    t = Image.new("L", (200, 160))
    t.putdata([v for row in tall for v in row])
    save("tall_palette", t.quantize(32), "palette 200 x 160: resized (nearest)")
    save("tall_one_bit", t.point(lambda v: 255 if v > 90 else 0).convert("1"), "1-bit 200 x 160: resized (nearest)")
    thin = Image.new("L", (30, 3000))
    thin.putdata([(x * 8 + y) % 256 for y in range(3000) for x in range(30)])
    save("tall_thin", thin, "grey 30 x 3000: more than 100 times taller than wide (resized in two steps)")
    # By hand: bit depths and colour types Pillow cannot save.
    w, h = 64, 64
    px = grey(w, h, 7)
    write_png(d / "grey2.png", w, h, 2, 0, [pack_bits([v >> 6 for v in r], 2) for r in px])
    what["grey2"] = "grey 2-bit 64 x 64"
    write_png(d / "grey4.png", w, h, 4, 0, [pack_bits([v >> 4 for v in r], 4) for r in px])
    what["grey4"] = "grey 4-bit 64 x 64"
    write_png(d / "grey1.png", w, h, 1, 0, [pack_bits([1 if v > 128 else 0 for v in r], 1) for r in px])
    what["grey1"] = "grey 1-bit 64 x 64"
    write_png(d / "grey1_odd.png", 61, 64, 1, 0, [pack_bits([1 if v > 100 else 0 for v in r[:61]], 1) for r in px])
    what["grey1_odd"] = "grey 1-bit 61 x 64"
    write_png(d / "grey16.png", w, h, 16, 0, [sum(([v, (v * 7) % 256] for v in r), []) for r in px])
    what["grey16"] = "grey 16-bit 64 x 64 (mode I;16)"
    write_png(d / "grey16_low.png", w, h, 16, 0, [sum(([0, v] for v in r), []) for r in px])
    what["grey16_low"] = "grey 16-bit 64 x 64, values under 256"
    write_png(d / "rgb16.png", w, h, 16, 2, [sum(([v, 1, 255 - v, 2, (v * 3) % 256, 3] for v in r), []) for r in px])
    what["rgb16"] = "RGB 16-bit 64 x 64"
    write_png(d / "rgba16.png", w, h, 16, 6, [sum(([v, 1, 255 - v, 2, (v * 3) % 256, 3, 200, 4] for v in r), []) for r in px])
    what["rgba16"] = "RGBA 16-bit 64 x 64"
    write_png(d / "la16.png", w, h, 16, 4, [sum(([v, 1, (v * 5) % 256, 2] for v in r), []) for r in px])
    what["la16"] = "grey and alpha 16-bit 64 x 64"
    colours = [(i * 16, 255 - i * 16, (i * 37) % 256) for i in range(16)]
    plte = [c for rgb_ in colours for c in rgb_]
    for depth in (1, 2, 4):
        n = 1 << depth
        write_png(d / f"palette{depth}.png", w, h, depth, 3, [pack_bits([(v * n // 256) % n for v in r], depth) for r in px],
                  plte=plte[:n * 3])
        what[f"palette{depth}"] = f"palette {depth}-bit 64 x 64"
    write_png(d / "palette_short.png", w, h, 8, 3, [[(v // 64) % 6 for v in r] for r in px], plte=plte[:12])
    what["palette_short"] = "palette 8-bit with 4 entries only, indexes beyond it"
    (d / "not_a_png.txt").write_text("this is not a picture\n")
    what["not_a_png.txt"] = "a text file"
    return what


# tape mm, picture, scene, what it shows
CASES = []


def case(name, png, scene="normal", tape=12, shows=""):
    CASES.append((name, png, scene, tape, shows))


def build_cases(what):
    for n in ["rgb", "rgba", "grey", "one_bit", "palette", "palette_trns", "la", "hub_label", "wide", "short",
              "tall_rgb", "tall_rgba", "tall_grey", "tall_la", "tall_palette", "tall_one_bit", "tall_thin",
              "grey2", "grey4", "grey1", "grey1_odd", "grey16", "grey16_low", "rgb16", "rgba16", "la16",
              "palette1", "palette2", "palette4", "palette_short"]:
        case(f"picture_{n}", f"{n}.png", shows=what[n] + ", 12 mm")
    case("tape_6mm", "grey.png", tape=6, shows="grey 100 x 64 on 6 mm tape: resized to 32 dots")
    case("tape_9mm", "grey.png", tape=9, shows="grey 100 x 64 on 9 mm tape: resized to 48 dots")
    case("tape_19mm", "tall_grey.png", tape=19, shows="grey 200 x 160 on 19 mm tape (96 dots): resized")
    case("tape_19mm_small", "hub_label.png", tape=19, shows="400 x 64 on 19 mm tape: 8 bytes a line")
    for scene in SCENES:
        if scene != "normal":
            case(f"scene_{scene}", "rgb.png", scene=scene, shows=f"scene {scene}")
    case("tape_24mm", "rgb.png", scene="single", tape=24, shows="unsupported tape size 24 mm")
    case("tape_none", "rgb.png", scene="single", tape=None, shows="no tape size given: labelle takes 12 mm")
    case("picture_missing", "missing.png", scene="single", shows="a picture that is not there")
    case("picture_text", "not_a_png.txt", scene="single", shows="a file that is no picture")


# ---------------------------------------------------------------------------------------
# The fake pyusb backend
# ---------------------------------------------------------------------------------------
ANSWER = [0x40, 0x00, 0x12, 0x4E, 0x0D, 0x6B, 0x00, 0x00]


def scene_text(name, devices):
    lines = [f"scene {name}"]
    for d in devices:
        flags = ",".join(sorted(d["flags"])) or "-"
        lines.append("dev {bus} {addr} {vid:04x} {pid:04x} {s0} {s1} {s2} {active} {flags}".format(
            bus=d["bus"], addr=d["addr"], vid=d["vid"], pid=d["pid"], s0=d["strings"][0], s1=d["strings"][1],
            s2=d["strings"][2], active=d["active"], flags=flags))
        for n, a, cls, bound, eps in d["ifaces"]:
            lines.append(f"iface {n} {a} {cls:02x} {'+' if bound else '-'} " +
                         ",".join(f"{e:02x}{'i' if k == 3 else 'b'}" for e, k in eps))
    return "\n".join(lines)


class Desc:
    """A descriptor: the given fields, 0 for any other pyusb reads."""
    def __init__(self, **fields): self.__dict__.update(fields)
    def __getattr__(self, name): return 0


class FakeBackend:
    """pyusb's IBackend for the devices of a scene: every operation besides reading descriptors
    recorded, each with the bus and address of its device."""

    def __init__(self, log, devices):
        import usb.core
        self.usb_core = usb.core
        self.log = log
        self.devices = devices
        self.state = {id(d): dict(active=d["active"], drivers={n: bound for n, a, c, bound, e in d["ifaces"]})
                      for d in devices}
        self.writes = 0
        self.reads = 0

    def tag(self, h): return f"{h['bus']}.{h['addr']}"
    def rec(self, h, text): self.log.append(f"{self.tag(h)} {text}")

    # Descriptors: read from what the kernel keeps, nothing on the wire; not recorded.
    def enumerate_devices(self): yield from self.devices
    def get_device_descriptor(self, d):
        return Desc(bLength=18, bDescriptorType=1, idVendor=d["vid"], idProduct=d["pid"], bNumConfigurations=1,
                    iManufacturer=d["strings"][0], iProduct=d["strings"][1], iSerialNumber=d["strings"][2],
                    bus=d["bus"], address=d["addr"])

    def groups(self, d):
        out = {}
        for n, a, cls, bound, eps in d["ifaces"]:
            out.setdefault(n, []).append((n, a, cls, bound, eps))
        return list(out.values())

    def get_configuration_descriptor(self, d, config):
        if config != 0: raise IndexError
        return Desc(bLength=9, bDescriptorType=2, bNumInterfaces=len(self.groups(d)), bConfigurationValue=1)

    def get_interface_descriptor(self, d, intf, alt, config):
        groups = self.groups(d)
        if intf >= len(groups) or alt >= len(groups[intf]): raise IndexError
        n, a, cls, bound, eps = groups[intf][alt]
        return Desc(bLength=9, bDescriptorType=4, bInterfaceNumber=n, bAlternateSetting=a, bInterfaceClass=cls,
                    bNumEndpoints=len(eps))

    def get_endpoint_descriptor(self, d, ep, intf, alt, config):
        n, a, cls, bound, eps = self.groups(d)[intf][alt]
        address, kind = eps[ep]
        return Desc(bLength=7, bDescriptorType=5, bEndpointAddress=address, bmAttributes=kind, wMaxPacketSize=64)

    # Everything else: recorded.
    def open_device(self, d):
        h = dict(d); h["_dev"] = d
        self.rec(h, "open_device"); return h
    def close_device(self, h): self.rec(h, "close_device")
    def get_configuration(self, h):
        v = self.state[id(h["_dev"])]["active"]; self.rec(h, f"get_configuration -> {v}"); return v
    def set_configuration(self, h, v):
        self.rec(h, f"set_configuration {v}")
        if "busy_setconfig" in h["flags"]: raise self.usb_core.USBError("Resource busy", -6, 16)
        if "denied_setconfig" in h["flags"]: raise self.usb_core.USBError("Insufficient permissions", -3, 13)
        self.state[id(h["_dev"])]["active"] = v
    def is_kernel_driver_active(self, h, i):
        if "nokernelapi" in h["flags"]:
            self.rec(h, f"is_kernel_driver_active {i} -> NotImplementedError"); raise NotImplementedError
        v = self.state[id(h["_dev"])]["drivers"][i]; self.rec(h, f"is_kernel_driver_active {i} -> {v}"); return v
    def detach_kernel_driver(self, h, i):
        self.rec(h, f"detach_kernel_driver {i}"); self.state[id(h["_dev"])]["drivers"][i] = False
    def claim_interface(self, h, i): self.rec(h, f"claim_interface {i}")
    def release_interface(self, h, i): self.rec(h, f"release_interface {i}")

    def write(self, kind, h, ep, data, timeout):
        self.rec(h, f"{kind}_write {ep:02x} {bytes(data).hex()} {timeout}")
        self.writes += 1
        for flag in h["flags"]:
            if flag.startswith("failwrite=") and self.writes == int(flag.split("=")[1]):
                raise self.usb_core.USBTimeoutError("Operation timed out", -7, 110)
        return len(data)

    def read(self, kind, h, ep, buff, timeout):
        self.rec(h, f"{kind}_read {ep:02x} {len(buff)} {timeout}")
        self.reads += 1
        for flag in h["flags"]:
            if flag.startswith("failread=") and self.reads == int(flag.split("=")[1]):
                raise self.usb_core.USBTimeoutError("Operation timed out", -7, 110)
        buff[:len(ANSWER)] = array.array("B", ANSWER)
        return len(ANSWER)

    def intr_write(self, h, ep, intf, data, timeout): return self.write("intr", h, ep, data, timeout)
    def bulk_write(self, h, ep, intf, data, timeout): return self.write("bulk", h, ep, data, timeout)
    def intr_read(self, h, ep, intf, buff, timeout): return self.read("intr", h, ep, buff, timeout)
    def bulk_read(self, h, ep, intf, buff, timeout): return self.read("bulk", h, ep, buff, timeout)

    def ctrl_transfer(self, h, bm, req, value, index, buff, timeout):
        self.rec(h, f"ctrl_transfer {bm:02x} {req:02x} {value:04x} {index:04x} {len(buff)} {timeout}")
        if bm & 0x80 and req == 6 and value >> 8 == 3:
            i = value & 0xFF
            if i == 0:
                if "nolangid" in h["flags"]: raise self.usb_core.USBError("Pipe error", -9, 32)
                data = [4, 3, 0x09, 0x04]
            else:
                text = f"DYMO string {i}".encode("utf-16-le")
                data = [2 + len(text), 3] + list(text)
            buff[:len(data)] = array.array("B", data)
            return len(data)
        raise self.usb_core.USBError("Pipe error", -9, 32)

    def __getattr__(self, name):
        def other(*args):
            self.log.append(f"{name} (not expected)")
        return other


# ---------------------------------------------------------------------------------------
# Real labelle
# ---------------------------------------------------------------------------------------
def check_labelle() -> None:
    """The labelle that runs is reference/labelle at LABELLE_COMMIT, unchanged."""
    head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=LABELLE, capture_output=True, text=True).stdout.strip()
    dirty = subprocess.run(["git", "status", "--porcelain", "src"], cwd=LABELLE, capture_output=True,
                           text=True).stdout.strip()
    if head != LABELLE_COMMIT or dirty:
        sys.exit(f"reference/labelle must be clean at {LABELLE_COMMIT} (is {head}, {dirty or 'clean'})")
    import labelle
    installed = Path(labelle.__file__).parent
    for rel in PORTED:
        a = hashlib.sha256((installed / rel).read_bytes()).hexdigest()
        b = hashlib.sha256((LABELLE / "src" / "labelle" / rel).read_bytes()).hexdigest()
        if a != b:
            sys.exit(f"installed labelle's {rel} differs from reference/labelle: reinstall it")


def run_labelle(scene: str, png: Path, tape) -> list[str]:
    """Real labelle's CLI on the fake backend: the operations, then the result line."""
    import platform
    import usb.core
    import usb.util
    from typer.testing import CliRunner
    from labelle.cli.cli import app

    log: list[str] = []
    fake = FakeBackend(log, SCENES[scene])
    real_find = usb.core.find
    usb.core.find = lambda **kw: real_find(backend=fake, **kw)
    real_system = platform.system
    platform.system = lambda: "Linux"
    try:
        args = ["--picture", str(png)] + (["--tape-size-mm", str(tape)] if tape is not None else [])
        result = CliRunner().invoke(app, args)
        text = result.output.strip()
        exc = result.exception
        if exc is not None and not isinstance(exc, SystemExit):
            outcome = f"error {exc}"
        elif text.startswith("Error:"):
            outcome = "error " + text[len("Error:"):].strip()
        else:
            outcome = "done"
        # The end of labelle's process: what an error leaves alive (its traceback keeps the frames,
        # and with them the devices) is finalized by pyusb's `weakref.finalize`, `atexit`: it
        # closes what is open. What was freed by the end of the command is closed already.
        import weakref
        weakref.finalize._exitfunc()
        weakref.finalize._shutdown = False
        result.exc_info = None
        del result, exc
        gc.collect()
    finally:
        usb.core.find = real_find
        platform.system = real_system
    log.append(outcome)
    return log


# ---------------------------------------------------------------------------------------
# What is extracted from labelle's source (ast)
# ---------------------------------------------------------------------------------------
def extract_constants() -> list[str]:
    """Constants and command bytes, read from labelle's source, one `name value` line each."""
    src = LABELLE / "src" / "labelle"
    rows = []
    consts = ast.parse((src / "lib/constants.py").read_text())
    unconfirmed = None
    values = {}
    for node in consts.body:
        if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name):
            name = node.targets[0].id
            if name == "UNCONFIRMED_MESSAGE":
                unconfirmed = ast.literal_eval(node.value)
            elif name in ("DEV_VENDOR", "PRINTER_INTERFACE_CLASS", "HID_INTERFACE_CLASS", "ESC", "SYN",
                          "DEFAULT_MARGIN_PX", "DPI", "MM_PER_INCH"):
                values[name] = ast.literal_eval(node.value)
            elif name == "SUPPORTED_PRODUCTS":
                for k, v in zip(node.value.keys, node.value.values):
                    if isinstance(v, ast.JoinedStr):
                        text = ""
                        for part in v.values:
                            text += part.value if isinstance(part, ast.Constant) else unconfirmed
                    else:
                        text = ast.literal_eval(v)
                    rows.append(f"SUPPORTED_PRODUCTS {ast.literal_eval(k):#06x} {text}")
    rows.append(f"UNCONFIRMED_MESSAGE {unconfirmed}")
    for k, v in values.items():
        rows.append(f"{k} {v}")
    labeler = ast.parse((src / "lib/devices/dymo_labeler.py").read_text())
    for cls in [n for n in labeler.body if isinstance(n, ast.ClassDef)]:
        for node in cls.body:
            if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name) and \
                    node.targets[0].id in ("SUPPORTED_TAPE_SIZES_MM", "DEFAULT_TAPE_SIZE_MM",
                                           "LABELER_DISTANCE_BETWEEN_PRINT_HEAD_AND_CUTTER_MM",
                                           "LABELER_PRINT_HEAD_HEIGHT_MM"):
                rows.append(f"{node.targets[0].id} {ast.literal_eval(node.value)}")
            if isinstance(node, ast.FunctionDef):
                # `cmd = [ESC, ord("X"), ...]`: the command letter of each method.
                for sub in ast.walk(node):
                    if isinstance(sub, ast.Assign) and isinstance(sub.targets[0], ast.Name) and sub.targets[0].id == "cmd" \
                            and isinstance(sub.value, ast.List) and len(sub.value.elts) >= 2:
                        e0, e1 = sub.value.elts[0], sub.value.elts[1]
                        if isinstance(e0, ast.Name) and e0.id == "ESC" and isinstance(e1, ast.Call) \
                                and isinstance(e1.func, ast.Name) and e1.func.id == "ord":
                            rows.append(f"COMMAND {node.name} ESC {ast.literal_eval(e1.args[0])}")
                # `self._maxLines = 200`, `synwait=64`
                for sub in ast.walk(node):
                    if isinstance(sub, ast.Assign) and isinstance(sub.targets[0], ast.Attribute) \
                            and sub.targets[0].attr == "_maxLines":
                        rows.append(f"_maxLines {ast.literal_eval(sub.value)}")
                    if isinstance(sub, ast.keyword) and sub.arg == "synwait" and isinstance(sub.value, ast.Constant):
                        rows.append(f"synwait {sub.value.value}")
    return sorted(set(rows))


ROW_TESTS = [
    ("SUPPORTED_PRODUCTS ", "row_supported_products"), ("UNCONFIRMED_MESSAGE ", "row_unconfirmed_message"),
    ("DEV_VENDOR ", "row_dev_vendor"), ("PRINTER_INTERFACE_CLASS ", "row_printer_interface_class"),
    ("HID_INTERFACE_CLASS ", "row_hid_interface_class"), ("ESC ", "row_esc"), ("SYN ", "row_syn"),
    ("DEFAULT_MARGIN_PX ", "row_default_margin_px"), ("DPI ", "row_dpi"), ("MM_PER_INCH ", "row_mm_per_inch"),
    ("SUPPORTED_TAPE_SIZES_MM ", "row_supported_tape_sizes"), ("DEFAULT_TAPE_SIZE_MM ", "row_default_tape_size"),
    ("LABELER_DISTANCE_BETWEEN_PRINT_HEAD_AND_CUTTER_MM ", "row_distance_head_cutter"),
    ("LABELER_PRINT_HEAD_HEIGHT_MM ", "row_print_head_height"),
    ("COMMAND _status_request ", "row_command_status_request"), ("COMMAND _tape_color ", "row_command_tape_color"),
    ("COMMAND _bytes_per_line ", "row_command_bytes_per_line"), ("_maxLines ", "row_max_lines"),
    ("synwait ", "row_synwait"),
]


def row_table(constants, ours_constants):
    """One line for each constant of labelle's source: where it is, ours, the Rust test, the result."""
    out = []
    for row in constants:
        test = next(t for p, t in ROW_TESTS if row.startswith(p))
        text = row if len(row) < 90 else row[:87] + "..."
        out.append(f"| `{text}` | `{test}` | {'**identical**' if row in ours_constants else '**DIFFERS**'} |")
    return out


def summary(ops: list[str]) -> str:
    writes = [o for o in ops if "_write " in o]
    reads = [o for o in ops if "_read " in o]
    ctrl = [o for o in ops if " ctrl_transfer " in o]
    status = sum(1 for o in writes if o.split()[3] == "1b41")
    closing = ops[-1] if ops else ""
    return f"{len(ops) - 1} operations: {len(writes)} writes ({status} × `1B 41`), {len(reads)} reads, " \
           f"{len(ctrl)} control reads; {closing[:60]}"


def main() -> int:
    if Path(sys.executable).resolve() != VENV_PY.resolve() and VENV_PY.exists():
        os.execv(str(VENV_PY), [str(VENV_PY), __file__])
    check_labelle()
    OUT.mkdir(parents=True, exist_ok=True)
    what = make_pngs(OUT / "pngs")
    build_cases(what)
    (OUT / "scenes.txt").write_text("\n".join(scene_text(n, d) for n, d in SCENES.items()) + "\n")
    with open(OUT / "cases.txt", "w") as f:
        for name, png, scene, tape, shows in CASES:
            f.write(f"case {name} {scene} {'none' if tape is None else tape} {png}\n")
    constants = extract_constants()
    (OUT / "constants.txt").write_text("\n".join(constants) + "\n")
    with open(OUT / "labelle.txt", "w") as f:
        for name, png, scene, tape, shows in CASES:
            path = OUT / "pngs" / png
            ops = run_labelle(scene, path, tape)
            # The fixtures folder is not part of the text.
            f.write(f"case {name}\n" + "".join(o.replace(str(OUT), "FIX") + "\n" for o in ops))
    for leftover in ("ours.txt", "ours-constants.txt"):
        (OUT / leftover).unlink(missing_ok=True)
    env = dict(os.environ, LABELLE_FIXTURES=str(OUT))
    cargo = ["cargo", "test", "-q", "-p", "maker-connect-engine", "--lib", "ports::labelle::tests::proof"]
    manifest = os.environ.get("LABELLE_MANIFEST")
    cmd = cargo if not manifest else ["cargo", "test", "-q", "--manifest-path", manifest, "--lib", "proof"]
    test = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env=env)
    if not (OUT / "ours.txt").exists():
        print(test.stdout[-4000:], test.stderr[-4000:])
        return 1
    theirs, ours = read_cases(OUT / "labelle.txt"), read_cases(OUT / "ours.txt")
    rows, bad = [], 0
    for name, png, scene, tape, shows in CASES:
        a, b = theirs.get(name, []), ours.get(name, [])
        if a == b:
            result = "**identical**"
        else:
            bad += 1
            at = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
            x = a[at][:70] if at < len(a) else "(end)"
            y = b[at][:70] if at < len(b) else "(end)"
            result = f"**DIFFERS** at operation {at + 1}: labelle `{x}`, ours `{y}`"
        rows.append(f"| `{name}` | {shows} | {summary(a)} | {result} |")
    ours_constants = (OUT / "ours-constants.txt").read_text().splitlines() if (OUT / "ours-constants.txt").exists() else []
    # `_dot_tab` ('B') and `_cut` ('E'): not ported, nothing that prints calls them.
    not_ported = ("COMMAND _dot_tab ", "COMMAND _cut ")
    constants = [c for c in constants if not c.startswith(not_ported)]
    const_bad = sorted(set(constants) ^ set(ours_constants))
    import PIL
    import usb
    table = "\n".join([
        BEGIN,
        f"labelle `{LABELLE_COMMIT[:7]}` with pyusb {usb.__version__} and Pillow {PIL.__version__}, run by "
        "`scripts/labelle-port-check.py`: labelle's own command line (`labelle --picture FILE --tape-size-mm N`) "
        "on a fake pyusb backend, and the Rust port on a recorder over the same devices. Every operation of "
        "both, in order, with the final result, is in `crates/engine/tests/labelle/labelle.txt` and `ours.txt`.",
        "",
        "| Example | What it shows | labelle (its own code, run) | Result |",
        "|---|---|---|---|",
        *rows,
        "",
        f"**{len(CASES) - bad} of {len(CASES)} examples identical.**",
        "",
        "### Constants, command bytes and sizes, extracted from labelle's source with `ast`",
        "",
        "| Row of labelle's source | Rust test | Result |",
        "|---|---|---|",
        *row_table(constants, ours_constants),
        "",
        f"**{len(constants) - len(set(constants) - set(ours_constants))} of {len(constants)} identical** "
        f"(rows only on one side: {len(const_bad)}).",
        END,
    ])
    if DOC.exists() and BEGIN in DOC.read_text():
        doc = DOC.read_text()
        DOC.write_text(doc[:doc.index(BEGIN)] + table + doc[doc.index(END) + len(END):])
    print(f"{len(CASES) - bad} of {len(CASES)} examples identical to labelle; "
          f"constants: {len(constants) - len(const_bad)} of {len(constants)} identical")
    for line in const_bad:
        print("constants differ:", line)
    return 0 if bad == 0 and not const_bad and test.returncode == 0 else 1


def read_cases(path: Path) -> dict[str, list[str]]:
    cases, name = {}, None
    for line in path.read_text().splitlines():
        if line.startswith("case "):
            name = line[5:]
            cases[name] = []
        elif line and name:
            cases[name].append(line)
    return cases


if __name__ == "__main__":
    sys.exit(main())
