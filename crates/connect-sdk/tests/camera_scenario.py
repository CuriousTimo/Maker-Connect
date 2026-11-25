"""What the SDK's camera modules (c0d7d233) do, for `tests/camera_reference.rs` to compare with
ours (`src/camera*.rs`, `src/configparser.rs`).

    python -I camera_scenario.py serve LOG        an HTTP server; prints its port, logs requests
    python -I camera_scenario.py run PORT LOG D   the steps, with the ini in folder D; prints JSON

The server is stdlib only and serves both sides: `PUT /c/snapshot` answers 403 to the token
"bad", else 204. Each request goes to LOG as one JSON line; a step reads and empties LOG.

The driver is `Fake`, a CameraDriver whose camera depends on its `path`: "/full" has every
capability, "/min" triggers and takes photos only, "/broken" cannot connect, "/slow" takes
0.3 s per photo, "/failset" fails to set its rotation. Every driver call goes to a log.

What a Python set or frozenset decides is compared without its order: the keys of a camera's
settings and of an ini section, capabilities, resolutions (see `src/camera.rs`).
"""

import json
import os
import sys
import threading
import time
# On Windows, pyos's model of CPython there (pyos_windows.py), before anything imports os.path.
if os.name == "nt":
    exec(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "pyos_windows.py")).read())

from configparser import ConfigParser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import MappingProxyType

from requests import Session

from prusa.connect.printer.camera import Camera, Resolution, Snapshot
from prusa.connect.printer.camera_configurator import CameraConfigurator
from prusa.connect.printer.camera_controller import CameraController
from prusa.connect.printer.camera_driver import CameraDriver
from prusa.connect.printer.const import CapabilityType as C
from prusa.connect.printer.models import CameraRegister

CALLS = []
SCAN = {}
FULL = [Resolution(640, 480), Resolution(1280, 720)]


class Fake(CameraDriver):
    name = "Fake"
    REQUIRES_SETTINGS = MappingProxyType({"path": "Path to the fake camera"})

    def __init__(self, camera_id, config, disconnected_cb):
        super().__init__(camera_id, config, disconnected_cb)
        path = config.get("path")
        self._capabilities = {C.TRIGGER_SCHEME, C.IMAGING}
        if path in ("/full", "/failset"):
            self._capabilities = set(C)
            self._available_resolutions = set(FULL)
            self._config["resolution"] = str(
                self._get_initial_resolution(self._available_resolutions,
                                             self._config))
            self._config.setdefault("focus", "0.5")

    @staticmethod
    def _scan():
        return {key: dict(value) for key, value in SCAN.items()}

    def _connect(self):
        CALLS.append(["connect", self.camera_id])
        if self._config.get("path") == "/broken":
            raise RuntimeError("no such camera")

    def _disconnect(self):
        CALLS.append(["disconnect", self.camera_id])

    def set_resolution(self, resolution):
        CALLS.append(["set_resolution", str(resolution)])

    def set_rotation(self, rotation):
        CALLS.append(["set_rotation", str(rotation)])
        if self._config.get("path") == "/failset":
            raise RuntimeError("the motor is stuck")

    def set_exposure(self, exposure):
        CALLS.append(["set_exposure", str(exposure)])

    def set_focus(self, focus):
        CALLS.append(["set_focus", str(focus)])

    def take_a_photo(self):
        CALLS.append(["take_a_photo", self.camera_id])
        if self._config.get("path") == "/slow":
            time.sleep(0.3)
        return b"JPEG" + self._config.get("path", "").encode()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    log_path = None

    def do_PUT(self):  # noqa: N802
        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length)
        headers = sorted([k.lower(), v] for k, v in self.headers.items())
        with open(self.log_path, "a", encoding="utf-8") as f:
            f.write(json.dumps({"method": "PUT", "path": self.path,
                                "headers": headers, "body": body.hex()}) + "\n")
        status = 403 if self.headers.get("Token") == "bad" else 204
        self.send_response(status)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def log_message(self, *args):
        pass


def serve(log):
    Handler.log_path = log
    open(log, "w", encoding="utf-8").close()
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(server.server_address[1], flush=True)
    server.serve_forever()


def err(exc):
    return [type(exc).__name__, str(exc)]


def calls():
    out = list(CALLS)
    CALLS.clear()
    return out


def requests_logged(log):
    with open(log, encoding="utf-8") as f:
        lines = [json.loads(line) for line in f if line.strip()]
    open(log, "w", encoding="utf-8").close()
    return lines


def value_str(value):
    """str() of a setting, a trigger scheme by its name"""
    return value.name if hasattr(value, "name") else str(value)


def settings(camera):
    return sorted([k, value_str(v)] for k, v in camera.get_settings().items())


def payload(camera):
    data = CameraRegister(camera).to_payload()
    return {
        "config": sorted([k, v] for k, v in data["config"].items()),
        "options": sorted([r["width"], r["height"]]
                          for r in data["options"]["available_resolutions"]),
        "capabilities": sorted(data["capabilities"]),
        "fingerprint": data["fingerprint"],
    }


def ini(path):
    parser = ConfigParser(interpolation=None)
    parser.read(path)
    return [[s, sorted(parser.items(s))] for s in parser.sections()]


def wait_ready(camera):
    camera.wait_ready(timeout=5)


def new_camera(path, disconnected):
    driver = Fake("id" + path.strip("/"), {
        "name": path, "driver": "Fake", "path": path}, disconnected.append)
    driver.connect()
    return Camera(driver)


SETTER_CASES = [
    ["rotation", 90], ["rotation", 45], ["rotation", "90"], ["rotation", 90.0],
    ["rotation", True], ["rotation", [90]], ["rotation", 0],
    ["exposure", 1], ["exposure", 1.5], ["exposure", 3], ["exposure", "1"],
    ["exposure", -2], ["exposure", None],
    ["focus", 0.5], ["focus", 0.25], ["focus", 2],
    ["resolution", {"width": 640, "height": 480}],
    ["resolution", {"width": 100, "height": 100}],
    ["resolution", "640x480"], ["resolution", {"width": 640}],
    ["resolution", {"width": 640, "height": 480, "x": 1}],
    ["trigger_scheme", "TEN_SEC"], ["trigger_scheme", "Every 10 seconds"],
    ["trigger_scheme", "MANUAL"], ["trigger_scheme", 5], ["trigger_scheme", [1]],
    ["name", "Renamed"], ["token", "t0k"],
]

FROM_STRING_CASES = [
    {"resolution": "640x480"}, {"resolution": " 640 x 480 "}, {"resolution": "640"},
    {"resolution": "1x2x3"}, {"resolution": "ax2"}, {"resolution": ""},
    {"rotation": " 90 "}, {"rotation": "9_0"}, {"rotation": "90.0"},
    {"exposure": "1e0"}, {"exposure": "nan"}, {"exposure": " -1.50 "},
    {"focus": "inf"}, {"focus": "x"},
    {"trigger_scheme": "BOGUS"}, {"trigger_scheme": "EACH_LAYER"},
    {"name": "n", "path": "/p"},
]

INITIAL_RESOLUTION_CASES = [
    {}, {"resolution": "640x480"}, {"resolution": "100x100"}, {"resolution": "bad"},
    {"resolution": "1x2x3"},
]


def run(port, log, folder):
    server = f"http://127.0.0.1:{port}"
    out = {}

    # --- one camera, its settings and registration ---
    disconnected = []
    camera = new_camera("/full", disconnected)
    out["full"] = {
        "calls": calls(), "settings": settings(camera), "payload": payload(camera),
        "output_resolution": str(camera.output_resolution),
        "string": sorted(Camera.string_from_settings(camera.get_settings()).items()),
    }
    minimal = new_camera("/min", disconnected)
    out["min"] = {"calls": calls(), "settings": settings(minimal),
                  "payload": payload(minimal),
                  "output_resolution": str(minimal.output_resolution)}
    try:
        minimal.set_value(C.ROTATION, 90)
        out["min_rotation"] = None
    except Exception as e:  # noqa: BLE001
        out["min_rotation"] = err(e)

    # --- the setters, each on a camera of its own ---
    cases = []
    for key, value in SETTER_CASES:
        disconnected = []
        camera = new_camera("/full", disconnected)
        calls()
        record = {"case": [key, value]}
        try:
            camera.set_settings(Camera.settings_from_json({key: value}),
                                store=False)
            record["error"] = None
        except Exception as e:  # noqa: BLE001
            record["error"] = err(e)
        record["calls"] = calls()
        record["settings"] = settings(camera)
        record["disconnected"] = len(disconnected)
        record["busy"] = camera.is_busy
        cases.append(record)
    disconnected = []
    camera = new_camera("/failset", disconnected)
    calls()
    try:
        camera.set_settings({"rotation": 90}, store=False)
        error = None
    except Exception as e:  # noqa: BLE001
        error = err(e)
    cases.append({"case": ["failset", 90], "error": error, "calls": calls(),
                  "settings": settings(camera),
                  "disconnected": len(disconnected), "busy": camera.is_busy})
    out["setters"] = cases

    # --- conversions ---
    records = []
    for case in FROM_STRING_CASES:
        try:
            converted = Camera.settings_from_string(case)
            records.append([[k, value_str(v)] for k, v in converted.items()])
        except Exception as e:  # noqa: BLE001
            records.append(err(e))
    out["from_string"] = records
    records = []
    for case in INITIAL_RESOLUTION_CASES:
        records.append(str(CameraDriver._get_initial_resolution(set(FULL), case)))
    try:
        CameraDriver._get_initial_resolution(set(), {})
    except Exception as e:  # noqa: BLE001
        records.append(err(e))
    out["initial_resolution"] = records

    # --- hashes, validation, scanning ---
    out["hashes"] = [
        CameraDriver.make_hash("/dev/video0"),
        Fake.get_config_hash({"name": "n", "driver": "Fake", "path": "/x"}),
        Fake.get_config_hash({"name": "n", "driver": "Fake"}),
        Fake.is_config_valid({"name": "n", "driver": "Fake", "path": "/x"}),
        Fake.is_config_valid({"name": "n", "path": "/x"}),
        sorted(Fake.get_required_settings()),
    ]
    SCAN.clear()
    SCAN.update({"/dev/a": {"name": "A", "path": "/full"}, "/dev/b": {"name": "B"},
                 "/dev/c": {"name": "C", "path": "/min", "trigger_scheme": "x"}})
    out["scan"] = [[k, sorted(v.items())] for k, v in Fake.scan().items()]

    # --- snapshots ---
    requests_logged(log)
    session = Session()
    shot = Snapshot()
    shot.camera_id, shot.camera_fingerprint, shot.camera_token = "c", "f" * 64, "tok"
    shot.printer_uuid, shot.timestamp, shot.data = "u u/1", 1763600000.1, b"\xff\xd8JPEG"
    first = shot.send(session, server).status_code
    incomplete = Snapshot()
    incomplete.data = b""
    second = incomplete.send(session, server).status_code
    out["snapshots"] = {"status": [first, second], "sent": requests_logged(log)}

    # --- the controller ---
    sent = []
    controller = CameraController(session, server, sent.append)
    disconnected = []
    camera = new_camera("/full", disconnected)
    controller.add_camera(camera)
    controller.register_camera(camera.camera_id)
    controller.register_camera("unknown")
    controller.tick()
    calls()
    camera.set_settings({"trigger_scheme": Camera.settings_from_json(
        {"trigger_scheme": "EACH_LAYER"})["trigger_scheme"]}, store=False)
    for _ in range(5):
        controller.layer_trigger()
        wait_ready(camera)
    layer_calls = calls()
    camera.set_settings({"trigger_scheme": Camera.settings_from_json(
        {"trigger_scheme": "FIFTH_LAYER"})["trigger_scheme"]}, store=False)
    for _ in range(10):
        controller.layer_trigger()
        wait_ready(camera)
    fifth_calls = calls()
    slow = new_camera("/slow", disconnected)
    controller.add_camera(slow)
    slow.set_settings({"trigger_scheme": Camera.settings_from_json(
        {"trigger_scheme": "MANUAL"})["trigger_scheme"]}, store=False)
    calls()
    controller.trigger_pile(Camera.settings_from_json(
        {"trigger_scheme": "MANUAL"})["trigger_scheme"])
    controller.trigger_pile(Camera.settings_from_json(
        {"trigger_scheme": "MANUAL"})["trigger_scheme"])
    wait_ready(slow)
    busy_calls = calls()
    camera._token = "bad"
    stored = []
    camera._driver.store_cb = stored.append
    loop = threading.Thread(target=controller.snapshot_loop, daemon=True)
    loop.start()
    requests_logged(log)
    camera.trigger_a_photo()
    deadline = time.time() + 5
    while camera.token is not None and time.time() < deadline:
        time.sleep(0.01)
    controller.stop()
    loop.join()
    out["controller"] = {
        "register": [payload(item.camera) for item in sent],
        "layer_calls": layer_calls, "fifth_calls": fifth_calls,
        "busy_calls": busy_calls, "token": camera.token,
        "stored": stored, "sent": requests_logged(log),
        "in_order": [c.camera_id for c in controller.cameras_in_order],
    }

    # --- the configurator ---
    path = os.path.join(folder, "cameras.ini")
    with open(path, "w", encoding="utf-8") as f:
        f.write("[camera_order]\n1 = gone1\n2 = gone2\n3 = storedA\n4 = storedB\n\n"
                "[camera::storedA]\nname = Stored A\ndriver = Fake\npath = /full\n\n"
                "[camera::storedB]\nname = Stored B\ndriver = Fake\npath = /broken\n"
                "resolution = 640x480\n\n"
                "[camera::bad]\nname = Bad\ndriver = Nope\n")
    SCAN.clear()
    SCAN.update({"/dev/video9": {"name": "Detected", "path": "/min"}})
    config = ConfigParser(interpolation=None)
    config.read(path)
    controller = CameraController(session, server, sent.append)
    configurator = CameraConfigurator(controller, config, path, [Fake])
    steps = []

    def record(name, error=None):
        for _, driver in configurator.loaded.items():
            if driver.is_connected:
                controller.get_camera(driver.camera_id).wait_ready(timeout=5)
        steps.append({
            "step": name, "error": error, "ini": ini(path),
            "order": list(configurator.order), "stored": sorted(configurator.stored),
            "detected": sorted(configurator.detected),
            "loaded": list(configurator.loaded),
            "connected": [configurator.is_connected(i) for i in configurator.loaded],
            "in_order": [c.camera_id for c in controller.cameras_in_order],
            "calls": calls(),
        })

    def attempt(name, fn):
        try:
            fn()
            record(name)
        except Exception as e:  # noqa: BLE001
            record(name, err(e))

    record("init")
    manual = {"name": "Manual", "driver": "Fake", "path": "/min"}
    attempt("add", lambda: configurator.add_camera("manual1", dict(manual)))
    attempt("add_again", lambda: configurator.add_camera("manual1", dict(manual)))
    attempt("add_invalid", lambda: configurator.add_camera("x", {"name": "x"}))
    attempt("set_order", lambda: configurator.set_order(["manual1", "nope"]))
    attempt("token", lambda: controller.get_camera("storedA").set_token("tok"))
    attempt("rename", lambda: controller.get_camera("storedA").set_settings(
        {"name": "A again"}))
    attempt("reset", lambda: configurator.reset_to_defaults("storedA"))
    detected_id = CameraDriver.make_hash("/dev/video9")
    attempt("remove_detected", lambda: configurator.remove_camera(detected_id))
    attempt("remove", lambda: configurator.remove_camera("manual1"))
    attempt("reload", lambda: configurator.load_cameras())
    attempt("reload_configs", lambda: configurator.load_cameras(load_configs=True))
    out["configurator"] = steps

    print(json.dumps(out))


if __name__ == "__main__":
    if sys.argv[1] == "serve":
        serve(sys.argv[2])
    else:
        run(int(sys.argv[2]), sys.argv[3], sys.argv[4])
