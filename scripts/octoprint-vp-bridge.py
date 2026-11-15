#!/usr/bin/env python3
"""The bridge between our Rust port and OctoPrint's own virtual printer (docs/audits/octoprint-marlin.md).

A PURE BYTE PASS-THROUGH: it makes the `VirtualPrinter` of `plugins/virtual_printer/virtual.py` (unchanged),
through the plugin's own factory with the same settings the reference run uses (`octoprint_proof_env.py`),
and passes on the calls the Rust port's serial object makes: `write(bytes)`, `readline()`, setting the
timeout, `close()`. No filtering, no retry, no timing of its own, no recording (the Rust side records).

    octoprint-vp-bridge.py SCENARIOS.json NAME READ_TIMEOUT BAUDRATE

Protocol, one line each way, ids chosen by the caller (each request is answered once, in any order):

    request   <id> W <hex bytes>      -> <id> ok <number written>          (VirtualPrinter.write)
              <id> R                  -> <id> ok <hex bytes>               (VirtualPrinter.readline; empty line: "ok ")
              <id> T <float>          -> <id> ok                           (VirtualPrinter.timeout = value)
              <id> C                  -> <id> ok                           (VirtualPrinter.close())
    an exception of the call: <id> err <class name> <hex message>

`write` and `readline` each run on a thread of their own (a `readline` waits for up to the read timeout,
and a `write` must not wait behind it, as with a serial port).

Run with `reference/.venv-octoprint-313/bin/python`.
"""

import json
import os
import queue
import sys
import threading

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import octoprint_proof_env as env  # noqa: E402


def main():
    scenarios_path, name, read_timeout, baudrate = sys.argv[1], sys.argv[2], float(sys.argv[3]), int(sys.argv[4])
    scenarios = env.load_config(scenarios_path)
    scn = next(s for s in scenarios["scenarios"] if s["name"] == name)
    config = env.scenario_config(scenarios, scn)

    from octoprint.printer.profile import PrinterProfileManager

    settings, pm, _ = env.setup(config)
    env.finish_plugins(settings, pm, {"plugin_manager": pm, "printer_profile_manager": PrinterProfileManager()})
    vp = env.make_virtual_printer(pm, read_timeout=read_timeout, baudrate=baudrate)

    out_lock = threading.Lock()

    def answer(line):
        with out_lock:
            sys.stdout.write(line + "\n")
            sys.stdout.flush()

    def run(op, rid, arg):
        try:
            if op == "W":
                result = str(vp.write(bytes.fromhex(arg)))
            elif op == "R":
                result = vp.readline().hex()
            elif op == "T":
                vp.timeout = float(arg)
                result = ""
            elif op == "C":
                vp.close()
                result = ""
            else:
                raise ValueError(f"unknown request {op}")
            answer(f"{rid} ok {result}")
        except Exception as e:  # noqa: BLE001
            answer(f"{rid} err {type(e).__name__} {str(e).encode('utf-8').hex()}")

    writes, reads = queue.Queue(), queue.Queue()

    def worker(q):
        while True:
            item = q.get()
            if item is None:
                return
            run(*item)

    for q in (writes, reads):
        threading.Thread(target=worker, args=(q,), daemon=True).start()

    answer("0 ok ready")
    for line in sys.stdin:
        parts = line.rstrip("\n").split(" ", 2)
        rid, op = parts[0], parts[1]
        arg = parts[2] if len(parts) > 2 else ""
        if op == "W":
            writes.put((op, rid, arg))
        elif op == "R":
            reads.put((op, rid, arg))
        else:
            run(op, rid, arg)
    os._exit(0)


if __name__ == "__main__":
    main()
