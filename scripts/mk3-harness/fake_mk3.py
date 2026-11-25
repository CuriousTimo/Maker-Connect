#!/usr/bin/env python3
"""A fake Prusa MK3S on a pseudo-terminal, for running PrusaLink and our agent against the
same printer. Every line a host sends is recorded with its time; the answers come from
Prusa-Firmware 3.14.1 (reference/Prusa-Firmware), each with its source in ANSWERS.

Like a real MK3 on USB, it starts over when the port is opened (DTR resets the board) and
says what setup() says (Marlin_main.cpp: "start", then "echo: <version>").

Usage: fake_mk3.py <link-file> <log-file> <seconds>
  writes the pty's path to <link-file>, records to <log-file>, stops after <seconds>.
"""
import os, pty, select, sys, time



# The printer this fake is: an MK3S without MMU on a release build of 3.14.1. Example values
# where the firmware reads its own EEPROM (serial number, nozzle), the same for every host.
FW_VERSION = "3.14.1"  # FW_VERSION, Configuration.h:32
FW_COMMITNR = 8237  # an example build number
FW_VERSION_FULL = f"{FW_VERSION}-{FW_COMMITNR}"  # Configuration.h:33
SERIAL_NUMBER = "CZPX1234X004XC12345"  # EEPROM_PRUSA_SN
NOZZLE_UM = 400  # EEPROM_NOZZLE_DIAMETER_uM
PRINTER_TYPE = 302  # nPrinterType(): PRINTER_MK3S
FW_COMMIT_HASH = "74a577bc0"  # an example hash


TEMPS = "T:20.2 /0.0 B:19.1 /0.0 T0:20.2 /0.0 @:0 B@:0 P:19.8 A:26.4"  # gcode_M105, its doc
POSITION = "X:0.00 Y:0.00 Z:0.00 E:0.00 Count X: 0.00 Y:0.00 Z:0.00 E:0.00"  # gcode_M114
FANS = "E0:0 RPM PRN1:0 RPM E0@:0 PRN1@:0"  # gcode_M123
MACHINE_NAME = "Prusa i3 MK3S"  # CUSTOM_MENDEL_NAME, variants/MK3S.h
EEPROM = bytearray(4096)  # EEPROM_SIZE; read as zeros, the same for every host


class Printer:
    """What the firmware keeps between lines."""

    def __init__(self):
        self.report_every = 0  # M155 S: seconds, 0 = off
        self.report_mask = 1   # M155 C: bit 0 temps, 1 fans, 2 position
        self.speed = 100       # M220
        self.flow = 100        # M221


def answers(printer: Printer, line: str) -> list[str]:
    """What the firmware answers a line (without its N and checksum): its output, then the
    "ok" every command ends with (ClearToSend). Unknown: only "ok"."""
    word = line.split(" ", 1)[0].upper()
    rest = line[len(word):].strip()
    def value(letter):
        for part in rest.split():
            if part[:1].upper() == letter and part[1:]:
                return part[1:]
        return None
    if word == "PRUSA":
        if rest.startswith("Fir"):  # Marlin_main.cpp:3980
            return [FW_VERSION_FULL, "ok"]
        if rest.startswith("SN"):  # Marlin_main.cpp:3973
            return [SERIAL_NUMBER, "ok"]
        return ["ok"]
    if word == "M862.1" and "Q" in rest:  # Marlin_main.cpp:7468, a float, 2 decimals
        return [f"{NOZZLE_UM / 1000:.2f}", "ok"]
    if word == "M862.2" and "Q" in rest:  # Marlin_main.cpp:7480
        return [str(PRINTER_TYPE), "ok"]
    if word == "M862.3" and "Q" in rest:  # sPrinterType
        return ["MK3S", "ok"]
    if word == "M862.4" and "Q" in rest:  # FW_VERSION_STR_P
        return [FW_VERSION, "ok"]
    if word == "M862.5" and "Q" in rest:  # GCODE_LEVEL
        return ["1", "ok"]
    if word == "M115":  # Marlin_main.cpp:5811, extended_capabilities_report:3578
        if "V" in rest:
            return [FW_VERSION, "ok"]
        return [f"FIRMWARE_NAME:Prusa-Firmware {FW_VERSION}+{FW_COMMITNR}_{FW_COMMIT_HASH} based on "
                f"Marlin FIRMWARE_URL:https://github.com/prusa3d/Prusa-Firmware PROTOCOL_VERSION:1.0 "
                f"MACHINE_TYPE:{MACHINE_NAME} EXTRUDER_COUNT:1",
                "Cap:AUTOREPORT_TEMP:1", "Cap:AUTOREPORT_FANS:1", "Cap:AUTOREPORT_POSITION:1",
                "Cap:EXTENDED_M20:1", "Cap:PRUSA_MMU2:1", "ok"]
    if word == "M105":  # Marlin_main.cpp:5379: "ok " first
        return ["ok " + TEMPS]
    if word in ("M420", "G81"):  # gcode_G81_M420 (Marlin_main.cpp:2897): no mesh after boot
        return ["Mesh bed leveling not active.", "ok"]
    if word == "M114":
        return [POSITION, "ok"]
    if word == "M123":
        return [FANS, "ok"]
    if word == "M155":  # Marlin_main.cpp:5409
        if value("S") is not None:
            printer.report_every = int(float(value("S")))
        printer.report_mask = int(value("C")) if value("C") is not None else 1
        return ["ok"]
    if word == "M220":  # Marlin_main.cpp:6397
        if value("S") is None:
            return [f"{printer.speed}%", "ok"]
        printer.speed = int(float(value("S")))
        return ["ok"]
    if word == "M221":  # Marlin_main.cpp:6431
        if value("S") is None:
            return [f"{printer.flow}%", "ok"]
        printer.flow = int(float(value("S")))
        return ["ok"]
    if word == "M27":  # cardreader.cpp:576, nothing printing
        return ["Not SD printing", "ok"]
    if word == "M21":  # CardReader::mount, cardreader.cpp:231: a card in
        return ["echo:SD card ok", "ok"]
    if word == "M20":  # Marlin_main.cpp:4796, an empty card
        return ["Begin file list", "End file list", "ok"]
    if word == "M73" and not rest:  # Marlin_main.cpp:5203, nothing printing (-1)
        return [f"{m} MODE: Percent done: -1; print time remaining in mins: -1; Change in mins: -1"
                for m in ("NORMAL", "SILENT")] + ["ok"]
    if word == "D3":  # Dcodes.cpp dcode_core + print_mem: 16 bytes a line
        a = rest.split()
        addr = next((int(x[2:], 16) for x in a if x.upper().startswith("AX")), 0)
        count = next((int(x[1:]) for x in a if x.upper().startswith("C")), 0)
        if any(x.upper().startswith("X") for x in a):
            return ["ok"]
        out = []
        while count > 0:
            n = min(16, count)
            # print_hex_word, putchar(' '), then per byte putchar(' ') + print_hex_byte
            out.append(f"{addr:04x} " + "".join(f" {b:02x}" for b in EEPROM[addr:addr + n]))
            addr += n
            count -= n
        return out + ["ok"]
    return ["ok"]


def reports(printer: Printer) -> list[str]:
    """host_autoreport (Marlin_main.cpp:1650): temperatures, then position, then fans."""
    out = []
    if printer.report_mask & 1:
        out.append(TEMPS)
    if printer.report_mask & 4:
        out.append(POSITION)
    if printer.report_mask & 2:
        out.append(FANS)
    return out


def strip(line: str) -> str:
    """The command of a numbered line: `N12 M105*34` -> `M105`."""
    if line.startswith("N") and " " in line:
        line = line.split(" ", 1)[1]
    return line.split("*", 1)[0].strip()


def main():
    link, logfile, seconds = sys.argv[1], sys.argv[2], float(sys.argv[3])
    master, slave = pty.openpty()
    name = os.ttyname(slave)
    os.close(slave)
    os.set_blocking(master, False)
    with open(link, "w") as f:
        f.write(name)
    log = open(logfile, "w", buffering=1)
    t0 = time.monotonic()
    def say(text):
        data = (text + "\n").encode()
        while data:
            select.select([], [master], [], 1.0)
            try:
                data = data[os.write(master, data):]
            except BlockingIOError:
                pass
        log.write(f"{time.monotonic() - t0:9.3f} TX {text}\n")
    opened, buf, boot_at = False, b"", None
    printer, next_report = Printer(), None
    while time.monotonic() - t0 < seconds:
        select.select([master], [], [], 0.05)
        try:
            data = os.read(master, 4096)
            if data == b"":  # nobody has the port open
                if opened:
                    log.write(f"{time.monotonic() - t0:9.3f} -- closed\n")
                opened = False
                time.sleep(0.05)
                continue
        except BlockingIOError:
            data = b""
        if not opened:
            opened = True
            boot_at = time.monotonic() + 1.0  # the board boots after the DTR reset
            log.write(f"{time.monotonic() - t0:9.3f} -- opened\n")
            buf = b""
        if boot_at and time.monotonic() >= boot_at:
            boot_at = None
            printer = Printer()  # a fresh board
            say("start")
            say("echo: " + FW_VERSION_FULL + "_" + FW_COMMIT_HASH)  # Marlin_main.cpp setup()
        buf += data
        while b"\n" in buf:
            raw, buf = buf.split(b"\n", 1)
            line = raw.decode(errors="replace").strip()
            if not line:
                continue
            log.write(f"{time.monotonic() - t0:9.3f} RX {line}\n")
            if boot_at is None:
                for a in answers(printer, strip(line)):
                    say(a)
        now = time.monotonic()
        if opened and boot_at is None and printer.report_every:
            if next_report is None or now >= next_report:
                if next_report is not None:
                    for r in reports(printer):
                        say(r)
                next_report = now + printer.report_every
        else:
            next_report = None


if __name__ == "__main__":
    main()
