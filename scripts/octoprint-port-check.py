#!/usr/bin/env python3
"""The Marlin port against OctoPrint 1.11.8, mechanically (docs/audits/octoprint-marlin.md).

The one reference is OctoPrint 1.11.8 (commit 42be740, AGPL-3.0) in reference/OctoPrint-1.11.8.
The test device is OctoPrint's own `plugins/virtual_printer/virtual.py`, run unchanged in Python
(reference/.venv-octoprint-313); nothing of it is ported to Rust. Both sides talk to the same Python
`VirtualPrinter`:

* the reference: OctoPrint's `Printer` (printer/standard.py) and `MachineCom` (util/comm.py), headless,
  in this process, with the virtual printer as the serial object (the plugin's own factory hook);
* ours: the Rust port (`OctoPrintPort`, driven by `Order`s) over a pipe to `octoprint-vp-bridge.py`,
  a pure byte pass-through to the same `VirtualPrinter`.

Each scenario is in `crates/engine/tests/octoprint/scenarios.json` (frozen, read by both sides) and runs
in a process of its own. Both sides record, in order, every byte written to and every line read from the
virtual printer.

    octoprint-port-check.py reference   run every scenario REPS times against the reference alone, write
                                        crates/engine/tests/octoprint/reference/<scenario>.json
    octoprint-port-check.py reference-bridge   the same, with the serial object a pipe to octoprint-vp-bridge.py
                                        (the experiment of the audit: crates/engine/tests/octoprint/reference-bridge/)
    octoprint-port-check.py bridge-check   the experiment's loud check: writes over the bridge == in-process, per scenario
    octoprint-port-check.py compare     compare them with ours (crates/engine/tests/octoprint/ours/, written
                                        by `cargo test -p maker-connect-engine octoprint_proof`) and write
                                        docs/audits/octoprint-marlin.md
    octoprint-port-check.py child NAME OUT    (internal) one reference run

Run with `reference/.venv-octoprint-313/bin/python` (it re-executes itself there if started with another).
"""

import concurrent.futures
import json
import os
import re
import subprocess
import sys
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
VENV_PY = os.path.join(ROOT, "reference", ".venv-octoprint-313", "bin", "python")
FIX = os.path.join(ROOT, "crates", "engine", "tests", "octoprint")
SCENARIOS = os.path.join(FIX, "scenarios.json")
REPS = 5

if os.path.realpath(sys.executable) != os.path.realpath(VENV_PY) and os.path.exists(VENV_PY):
    os.execv(VENV_PY, [VENV_PY] + sys.argv)

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import octoprint_proof_env as env  # noqa: E402


# ---------------------------------------------------------------------------------------------
# the recorder: the serial object comm opens (wraps the plugin's VirtualPrinter, forwards everything)
# ---------------------------------------------------------------------------------------------
class Recorder:
    """What `comm.py` holds as `self._serial`. It forwards every call and attribute to the
    `VirtualPrinter` and records the bytes of `write` and the non-empty results of `readline`."""

    def __init__(self, target):
        object.__setattr__(self, "_target", target)
        object.__setattr__(self, "writes", [])
        object.__setattr__(self, "reads", [])
        object.__setattr__(self, "cond", threading.Condition())
        object.__setattr__(self, "last", time.monotonic())

    def write(self, data):
        with self.cond:
            self.writes.append(env.lat1(bytes(data)))
            object.__setattr__(self, "last", time.monotonic())
            self.cond.notify_all()
        return self._target.write(data)

    def readline(self):
        line = self._target.readline()
        if line:
            with self.cond:
                self.reads.append(env.lat1(line))
                object.__setattr__(self, "last", time.monotonic())
                self.cond.notify_all()
        return line

    def __getattr__(self, name):
        return getattr(self._target, name)

    def __setattr__(self, name, value):
        setattr(self._target, name, value)

    def __str__(self):
        return str(self._target)

    # waits of the scenario steps, on what was recorded
    def wait_count(self, which, pattern, count, timeout=60.0):
        rx = re.compile(pattern)
        end = time.monotonic() + timeout
        with self.cond:
            while True:
                n = sum(1 for x in getattr(self, which) if rx.search(x))
                if n >= count:
                    return True
                left = end - time.monotonic()
                if left <= 0:
                    return False
                self.cond.wait(left)

    def settle(self, quiet_ms):
        """wait until nothing was written or read for `quiet_ms`, counted from the start of the step at
        the earliest (what an order just given has not yet put on the wire counts as not quiet)"""
        quiet = quiet_ms / 1000.0
        start = time.monotonic()
        while True:
            with self.cond:
                left = max(start, self.last) + quiet - time.monotonic()
            if left <= 0:
                return
            time.sleep(left)


# ---------------------------------------------------------------------------------------------
# the bridge experiment (`reference-bridge`): the same reference code, but the serial object it opens is a
# pipe to `octoprint-vp-bridge.py` (the process our Rust port talks to), spoken to the way the Rust `Bridge`
# (proof.rs) speaks to it. No line of OctoPrint's own code changes: only where `_open_serial` gets its
# serial object from, the same seam as the factory hook above.
# ---------------------------------------------------------------------------------------------
class BridgeSerial:
    def __init__(self, scenario, port, baud, read_timeout):
        import subprocess as sp

        bridge = os.path.join(os.path.dirname(os.path.abspath(__file__)), "octoprint-vp-bridge.py")
        object.__setattr__(self, "_p", sp.Popen(
            [sys.executable, bridge, SCENARIOS, scenario, str(read_timeout), str(baud)],
            stdin=sp.PIPE, stdout=sp.PIPE, stderr=sp.DEVNULL, text=True, bufsize=1))
        object.__setattr__(self, "_next", 1)
        object.__setattr__(self, "_waiting", {})
        object.__setattr__(self, "_lock", threading.Lock())
        object.__setattr__(self, "_port", port)
        object.__setattr__(self, "_baud", baud)
        object.__setattr__(self, "_timeout", read_timeout)
        ready = threading.Event()
        object.__setattr__(self, "_ready", ready)
        threading.Thread(target=self._reader, daemon=True).start()
        if not ready.wait(60):
            raise RuntimeError("the bridge did not say it was ready")

    def _reader(self):
        for line in self._p.stdout:
            parts = line.rstrip("\n").split(" ", 2)
            if len(parts) < 2 or not parts[0].isdigit():
                continue
            rid, status, rest = int(parts[0]), parts[1], parts[2] if len(parts) > 2 else ""
            if rid == 0:
                self._ready.set()
                continue
            with self._lock:
                slot = self._waiting.pop(rid, None)
            if slot:
                slot[1].append((status, rest))
                slot[0].set()
        with self._lock:
            pending = list(self._waiting.values())
            self._waiting.clear()
        for ev, box in pending:
            box.append(("err", "SerialException " + b"the bridge ended".hex()))
            ev.set()

    def _call(self, request):
        import serial

        ev, box = threading.Event(), []
        with self._lock:
            rid = self._next
            object.__setattr__(self, "_next", rid + 1)
            self._waiting[rid] = (ev, box)
        try:
            self._p.stdin.write(f"{rid} {request}\n")
            self._p.stdin.flush()
        except OSError:
            raise serial.SerialException("the bridge ended")
        ev.wait()
        status, rest = box[0]
        if status == "ok":
            return rest
        cls, _, msg = rest.partition(" ")
        raise serial.SerialException(bytes.fromhex(msg).decode("utf-8", "replace"))

    # what the Rust `SerialTransport` for the bridge offers
    @property
    def port(self):
        return self._port

    @property
    def baudrate(self):
        return self._baud

    @property
    def timeout(self):
        return self._timeout

    @timeout.setter
    def timeout(self, value):
        object.__setattr__(self, "_timeout", value)
        self._call(f"T {value}")

    def close(self):
        self._call("C")

    def write(self, data):
        return int(self._call("W " + bytes(data).hex()).strip())

    def readline(self):
        return bytes.fromhex(self._call("R"))

    def __str__(self):
        return f"VIRTUAL(read_timeout={self._timeout})"

    def kill(self):
        self._p.kill()


# ---------------------------------------------------------------------------------------------
# what OctoPrint's own code gets in place of its file manager and analysis queue (the Rust port's
# `LocalFileManager`: the job's file is a local path; no recovery data; no metadata)
# ---------------------------------------------------------------------------------------------
class LocalFileManager:
    def path_on_disk(self, destination, path):
        return path

    def path_in_storage(self, destination, path):
        return path

    def split_path(self, destination, path):
        folder, _, name = path.rpartition("/")
        return folder, name

    def get_recovery_data(self):
        return None

    def delete_recovery_data(self):
        pass

    def save_recovery_data(self, origin, path, pos):
        pass

    def log_print(self, *args, **kwargs):
        pass

    def get_metadata(self, *args, **kwargs):
        return None


class AnalysisQueue:
    def pause(self):
        pass

    def resume(self):
        pass


# ---------------------------------------------------------------------------------------------
# the scenarios
# ---------------------------------------------------------------------------------------------
def load_scenarios():
    with open(SCENARIOS) as f:
        return json.load(f)


def whole(v):
    return int(v) if isinstance(v, float) and v == int(v) and abs(v) < 9e15 else v


def run_reference(top, scn, files_dir):
    """One run of one scenario against OctoPrint's own Printer and MachineCom; returns the transcript"""
    import copy
    import tempfile

    settings, pm, basedir = env.setup(env.scenario_config(top, scn))

    # point `serialList` at an empty directory instead of /dev: the reference never looks at a real
    # device (our test does the same, `comm.rs: test_serial_dir`)
    import octoprint.util.comm as comm
    from octoprint.events import Events, eventManager
    from octoprint.printer.profile import PrinterProfileManager
    from octoprint.printer.standard import Printer

    empty_dev = tempfile.mkdtemp(prefix="octoprint-proof-dev-")
    real_scandir = os.scandir
    os.scandir = lambda path=".": real_scandir(empty_dev if path == "/dev" else path)

    recorder = []

    # the plugin's factory hook, wrapped so the serial object is recorded (the hook itself unchanged)
    key = "octoprint.comm.transport.serial.factory"
    hooks = pm._plugin_hooks[key]
    wrapped = []
    for order, name, callback in hooks:

        def make(cb):
            def factory(*args, **kwargs):
                if os.environ.get("OCTOPRINT_PROOF_BRIDGE"):
                    # args: comm_instance, port, baudrate, read_timeout (the plugin's factory signature)
                    if args[1] != "VIRTUAL":
                        return None
                    obj = BridgeSerial(scn["name"], args[1], args[2], float(args[3]))
                else:
                    obj = cb(*args, **kwargs)
                if obj is None:
                    return None
                rec = Recorder(obj)
                recorder.append(rec)
                return rec

            return factory

        wrapped.append((order, name, make(callback)))
    pm._plugin_hooks[key] = wrapped

    em = eventManager()
    em.fire(Events.STARTUP)

    ppm = PrinterProfileManager()
    fm = LocalFileManager()
    aq = AnalysisQueue()
    printer = Printer(fm, aq, ppm)
    components = {
        "plugin_manager": pm,
        "printer_profile_manager": ppm,
        "event_bus": em,
        "analysis_queue": aq,
        "file_manager": fm,
        "printer": printer,
    }
    env.finish_plugins(settings, pm, components)
    prompt = pm.get_plugin_info("action_command_prompt").implementation

    error = None
    command_id = [0]

    def rec():
        return recorder[-1] if recorder else None

    def wait_for(what, ok, timeout=60.0):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if ok():
                return True
            time.sleep(0.02)
        raise RuntimeError(f"timed out waiting for {what}")

    try:
        c = scn["connect"]
        printer.connect(port="VIRTUAL" if c.get("port") else None, baudrate=c.get("baud"))
        for step in scn["steps"]:
            try:
                do(step, printer, prompt, rec, wait_for, files_dir, command_id)
            except Refused:
                pass  # the API refuses, nothing is sent (the port reports it to the hub)
    except Exception as e:  # noqa: BLE001
        error = str(e)
    r = rec()
    return {
        "scenario": scn["name"],
        "writes": list(r.writes) if r else [],
        "reads": list(r.reads) if r else [],
        "error": error,
    }


def gcode_lines(text):
    """K1: each line as one Enter in the terminal: split at \\n, a trailing \\r dropped, empty ones skipped"""
    out = []
    for line in text.split("\n"):
        line = line[:-1] if line.endswith("\r") else line
        if line != "":
            out.append(line)
    return out


def do(step, printer, prompt, rec, wait_for, files_dir, command_id):
    if "settle" in step:
        # first the step's own condition, then the wire quiet
        wait_for("a recorder", lambda: rec() is not None)
        rec().settle(step["settle"])
    elif "wait_operational" in step:
        wait_for("operational", lambda: printer._comm is not None and printer._comm.isOperational())
    elif "wait_state" in step:
        want = step["wait_state"]
        wait_for(f"state {want}", lambda: printer._comm is not None and printer._comm.getStateString() == want)
    elif "wait_write" in step:
        w = step["wait_write"]
        if not rec().wait_count("writes", w["match"], w.get("count", 1)):
            raise RuntimeError(f"timed out waiting for a write matching \"{w['match']}\"")
    elif "wait_read" in step:
        w = step["wait_read"]
        if not rec().wait_count("reads", w["match"], w.get("count", 1)):
            raise RuntimeError(f"timed out waiting for a line matching \"{w['match']}\"")
    elif "command" in step:
        command_id[0] += 1
        command(step["command"], printer)
    elif "job" in step:
        command_id[0] += 1
        job(step["job"], printer, files_dir)
    elif "sd_job" in step:
        # #238: Order::SdJob is the files API's print of a card file, as the action sd_print (port.rs K7, K20)
        command_id[0] += 1
        command({"action": "sd_print", "file": step["sd_job"]["file"]}, printer)
    elif "control" in step:
        which = step["control"]
        tags = {"source:api", "api:job"}
        # job.py:48-68
        check_operational(printer)
        refuse_unless(printer.is_printing() or printer.is_paused(), "Printer is neither printing nor paused")
        if which == "pause":
            printer.pause_print(tags=tags)
        elif which == "resume":
            printer.resume_print(tags=tags)
        elif which == "stop":
            printer.cancel_print(tags=tags)
        else:
            raise RuntimeError(f"no control {which}")
    elif "answer" in step:
        # the prompt plugin's `select` (action_command_prompt/__init__.py:122), without the permission check
        choice = step["answer"]
        if prompt._prompt is None or not prompt._prompt.validate_choice(choice):
            raise RuntimeError("no such prompt choice")
        prompt._answer_prompt(choice)
    elif "close" in step:
        printer.disconnect()
    else:
        raise RuntimeError(f"unknown step {step}")


class Refused(Exception):
    """OctoPrint's own API (server/api/*.py) refuses the call: nothing is sent to the printer"""


def refuse_unless(ok, text):
    if not ok:
        raise Refused(text)


def check_printhead(printer, which):
    # printerPrintheadCommand (printer.py:302)
    refuse_unless(printer.is_operational() and not (printer.is_printing() and which != "feedrate"),
                  "Printer is not operational or currently printing")


def check_operational(printer):
    refuse_unless(printer.is_operational(), "Printer is not operational")


def check_machinecode_select(printer, filename):
    # gcodeFileCommand, select (files.py:877-888)
    from octoprint.filemanager import valid_file_type

    refuse_unless(valid_file_type(filename, type="machinecode"),
                  "Cannot select file for printing, not a machinecode file")
    refuse_unless(printer.is_ready(), "Printer is already printing, cannot select a new file")


def command(action, printer):
    """M1: each `Action` as the OctoPrint call the Rust top edge makes (`port.rs`, `carry_out`), after the
    refusals of the API endpoint that makes it (api_printer.rs: `printerPrintheadCommand`, `printerToolCommand`,
    `printerBedCommand`, `printerSdCommand`, `printerCommand`, `gcodeFileCommand`, `deleteGcodeFile`): a
    refused action sends nothing"""
    kind = action["action"]
    if kind == "gcode":
        check_operational(printer)
        lines = gcode_lines(action["text"])
        printer.commands(lines, tags={"source:api", "api:printer.command"})
    elif kind == "home":
        check_printhead(printer, "home")
        printer.home(list(action["axes"]), tags={"source:api", "api:printer.printhead"})
    elif kind == "jog":
        check_printhead(printer, "jog")
        axes = {k: whole(action[k]) for k in ("x", "y", "z") if action.get(k) is not None}
        speed = whole(action["feed"]) if action.get("feed") is not None else None
        printer.jog(axes, relative=True, speed=speed, tags={"source:api", "api:printer.printhead"})
    elif kind == "extrude":
        check_operational(printer)
        refuse_unless(not printer.is_printing(), "Printer is currently printing")
        printer.extrude(whole(action["mm"]), speed=None, tags={"source:api", "api:printer.tool"})
    elif kind == "temperature":
        check_operational(printer)
        heater = action["heater"]
        name = "tool0" if heater == "nozzle" else "bed"
        api = "api:printer.tool" if heater == "nozzle" else "api:printer.bed"
        printer.set_temperature(name, whole(action["celsius"]), tags={"source:api", api})
    elif kind in ("fan", "motors_off"):
        check_operational(printer)
        text = ("M106 S255" if action["on"] else "M106 S0") if kind == "fan" else "M18"
        printer.commands([text], tags={"source:api", "api:printer.command"})
    elif kind == "speed":
        check_printhead(printer, "feedrate")
        printer.feed_rate(action["percent"], tags={"source:api", "api:printer.printhead"})
    elif kind == "flow":
        check_operational(printer)
        printer.flow_rate(action["percent"], tags={"source:api", "api:printer.tool"})
    elif kind == "sd_list":
        refuse_unless(printer.is_operational() and not printer.is_printing() and not printer.is_paused(),
                      "Printer is not operational or currently busy")
        if printer.is_sd_ready():
            printer.refresh_sd_files(blocking=False, tags={"source:api", "api:printer.sd"})
    elif kind == "sd_print":
        file = action["file"]
        refuse_unless(file in [x["name"] for x in printer.get_sd_files()], "not found")
        check_machinecode_select(printer, file)
        refuse_unless(printer.is_operational(), "Printer is not operational, cannot directly start printing")
        printer.select_file(file, True, printAfterSelect=True, tags={"source:api", "api:files.sd"})
    elif kind == "sd_delete":
        file = action["file"]
        refuse_unless(file in [x["name"] for x in printer.get_sd_files()], "not found")
        selected = printer._selectedFile
        if selected and selected["sd"] and selected["filename"] == file:
            printer.unselect_file()
        printer.delete_sd_file(file, tags={"source:api", "api:files.sd"})
    else:
        # M2: no such control; nothing is sent
        raise Refused(f"OctoPrint has no such control: {kind}")


def job(order, printer, files_dir):
    """a job: `select_file(path, sd=False)` then `start_print` (K6: the file under the job's name), after the
    refusals of files.py (select) and job.py (start)"""
    path = os.path.join(files_dir, order["file"])
    tags = {"source:api", "api:job"}
    check_machinecode_select(printer, path)
    printer.select_file(path, False, False, None, None, tags=tags)
    check_operational(printer)
    refuse_unless(not (printer.is_printing() or printer.is_paused()),
                  "Printer already has an active print job, did you mean 'restart'?")
    printer.start_print(tags=tags)


# ---------------------------------------------------------------------------------------------
# the driver
# ---------------------------------------------------------------------------------------------
def child(name, out):
    scenarios = load_scenarios()
    scn = next(s for s in scenarios["scenarios"] if s["name"] == name)
    files_dir = os.path.join(FIX, "files")
    result = run_reference(scenarios, scn, files_dir)
    with open(out, "w") as f:
        json.dump(result, f)
    sys.stdout.flush()
    os._exit(0)  # threads of OctoPrint (daemon or not) end with the run


def only():
    """`MAKER_CONNECT_PROOF_ONLY=a,b`: run only these scenarios, the others' results kept as they are"""
    names = os.environ.get("MAKER_CONNECT_PROOF_ONLY")
    return set(names.split(",")) if names else None


def reference(subdir="reference"):
    scenarios = load_scenarios()
    chosen = only()
    if chosen is not None:
        scenarios = dict(scenarios, scenarios=[s for s in scenarios["scenarios"] if s["name"] in chosen])
    os.makedirs(os.path.join(FIX, subdir), exist_ok=True)
    for old in os.listdir(os.path.join(FIX, subdir)):
        if old.endswith(".runs.json") and (chosen is None or old[: -len(".runs.json")] in chosen):
            os.remove(os.path.join(FIX, subdir, old))  # the runs of a scenario that was not reproducible, from an earlier sample
    tmp = os.path.join(FIX, subdir, "_runs")
    os.makedirs(tmp, exist_ok=True)

    def one(name, rep):
        out = os.path.join(tmp, f"{name}.{rep}.json")
        subprocess.run([sys.executable, os.path.abspath(__file__), "child", name, out], check=False, timeout=600,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        with open(out) as f:
            return json.load(f)

    jobs = [(s["name"], r) for s in scenarios["scenarios"] for r in range(REPS)]
    results = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
        futures = {pool.submit(one, n, r): (n, r) for n, r in jobs}
        for fut, (n, r) in futures.items():
            try:
                results.setdefault(n, {})[r] = fut.result()
            except Exception as e:  # noqa: BLE001
                results.setdefault(n, {})[r] = {"scenario": n, "writes": [], "reads": [], "error": f"driver: {e}"}
    summary = {}
    if chosen is not None:
        with open(os.path.join(FIX, subdir, "summary.json")) as f:
            summary = json.load(f)
    for s in scenarios["scenarios"]:
        n = s["name"]
        runs = [results[n][r] for r in range(REPS)]
        norm = [normalise(s, r["reads"]) for r in runs]
        writes_same = all(r["writes"] == runs[0]["writes"] for r in runs)
        reads_same = all(x == norm[0] for x in norm)
        errors = [r["error"] for r in runs if r["error"]]
        reproducible = writes_same and reads_same
        summary[n] = {"reproducible": reproducible, "writes_same": writes_same, "reads_same": reads_same,
                      "errors": errors}
        first = dict(runs[0])
        first["reproducible"] = reproducible
        with open(os.path.join(FIX, subdir, f"{n}.json"), "w") as f:
            json.dump(first, f, indent=0)
        if not reproducible:
            with open(os.path.join(FIX, subdir, f"{n}.runs.json"), "w") as f:
                json.dump(runs, f, indent=0)
    with open(os.path.join(FIX, subdir, "summary.json"), "w") as f:
        json.dump(summary, f, indent=1)
    import shutil

    shutil.rmtree(tmp, ignore_errors=True)
    bad = [n for n, s in summary.items() if not s["reproducible"]]
    print(f"{len(summary)} scenarios, {len(summary) - len(bad)} reproducible, not reproducible: {bad}")


# ---------------------------------------------------------------------------------------------
# normalisation (the same rules for both sides; the Rust test applies the same ones)
# ---------------------------------------------------------------------------------------------
TEMP_RX = re.compile(r"(?<![A-Za-z0-9_])([TB]\d*:)\s*-?\d+(?:\.\d+)?")


def normalise(scn, reads):
    """(b): the lines read back, after dropping only `wait` lines and, in a scenario whose simulated
    temperatures are moving, the numbers behind `T:` and `B:`"""
    out = []
    for line in reads:
        if line.strip() == "wait":
            continue
        if scn.get("temperatures_moving"):
            line = TEMP_RX.sub(lambda m: m.group(1) + "N", line)
        out.append(line)
    return out


def bridge_check():
    """The bridge experiment's loud check: every scenario's writes over the bridge (`reference-bridge/`) are
    those of the in-process run (`reference/`), run for run; the lines read are compared too (the M112 echo is
    the one line that a thread race can decide). Exit 1 when a write differs."""
    scenarios = load_scenarios()
    bad = 0
    for scn in scenarios["scenarios"]:
        n = scn["name"]
        a = json.load(open(os.path.join(FIX, "reference", f"{n}.json")))
        b = json.load(open(os.path.join(FIX, "reference-bridge", f"{n}.json")))
        writes = a["writes"] == b["writes"]
        reads = normalise(scn, a["reads"]) == normalise(scn, b["reads"])
        early = any(w.strip().endswith("M105") or w.strip() == "M105" for w in b["writes"])
        print(f"{n:<24} writes {'IDENTICAL' if writes else 'DIFFER !!!'}  lines {'identical' if reads else 'differ'}  M105 written: {early}")
        bad += 0 if writes else 1
    print("LOUD: " + (f"{bad} scenario(s) WRITE differently over the bridge" if bad else "the writes over the bridge are identical in all scenarios"))
    sys.exit(1 if bad else 0)


def main():
    if len(sys.argv) >= 2 and sys.argv[1] == "bridge-check":
        bridge_check()
    elif len(sys.argv) >= 2 and sys.argv[1] == "reference":
        reference()
    elif len(sys.argv) >= 2 and sys.argv[1] == "reference-bridge":
        os.environ["OCTOPRINT_PROOF_BRIDGE"] = "1"  # the children inherit it
        reference("reference-bridge")
    elif len(sys.argv) >= 4 and sys.argv[1] == "child":
        child(sys.argv[2], sys.argv[3])
    elif len(sys.argv) >= 2 and sys.argv[1] == "compare":
        import octoprint_proof_compare as compare

        sys.exit(compare.main(load_scenarios(), FIX, normalise))
    else:
        print(__doc__)
        sys.exit(2)


if __name__ == "__main__":
    main()
