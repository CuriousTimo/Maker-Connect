"""What the SDK's download.py (c0d7d233) does, for `tests/download_reference.rs` to compare
with ours (`src/download.rs`).

    python -I download_scenario.py serve LOG      an HTTP server; prints its port, logs requests
    python -I download_scenario.py run PORT LOG D the steps in folder D; prints JSON

The server is stdlib only and serves both sides. Each request's headers go to LOG as one JSON
line; a step reads and empties LOG. A step's record: how `DownloadMgr.start` ended (the dict,
or [repr(err)]), the events the loop sent, the files left in D and the headers the server got.
D is written as "<D>"; the random `transfer_id` and the clock's fields are masked.
"""

import gzip
import hashlib
import json
import os
import sys
import threading
import time
import zlib
# On Windows, pyos's model of CPython there (pyos_windows.py), before anything imports os.path.
if os.name == "nt":
    exec(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "pyos_windows.py")).read())

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BODY = bytes(range(256)) * 4  # 1024 bytes
BIG = bytes((i * 7) % 251 for i in range(300_000))


def routes():
    return {
        "/ok.gcode": (200, [("Content-Type", "application/octet-stream")], BODY),
        "/plain.gcode": (200, [("Content-Type", "text/plain")], BODY),
        "/missing": (404, [("Content-Type", "text/html")], b"not here"),
        "/html": (200, [("Content-Type", "text/html")], BODY),
        "/charset": (200, [("Content-Type", "text/plain; charset=utf-8")], BODY),
        "/upper": (200, [("Content-Type", "Application/Octet-Stream")], BODY),
        "/no-type": (200, [], BODY),
        "/empty": (200, [("Content-Type", "application/octet-stream")], b""),
        "/gzip": (200, [("Content-Type", "application/octet-stream"),
                        ("Content-Encoding", "gzip")], gzip.compress(BIG, mtime=0)),
        "/deflate": (200, [("Content-Type", "application/octet-stream"),
                           ("Content-Encoding", "deflate")], zlib.compress(BIG)),
        "/big": (200, [("Content-Type", "application/octet-stream")], BIG),
    }


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    log_path = None

    def do_GET(self):  # noqa: N802
        with open(self.log_path, "a", encoding="utf-8") as f:
            headers = sorted([k.lower(), v] for k, v in self.headers.items())
            f.write(json.dumps({"path": self.path, "headers": headers}) + "\n")
        if self.path == "/chunked":
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Transfer-Encoding", "chunked")
            self.end_headers()
            for i in range(0, len(BIG), 70_000):
                part = BIG[i:i + 70_000]
                self.wfile.write(b"%x\r\n%s\r\n" % (len(part), part))
            self.wfile.write(b"0\r\n\r\n")
            return
        status, headers, body = routes().get(self.path, (404, [], b""))
        self.send_response(status)
        for k, v in headers:
            self.send_header(k, v)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def serve(log):
    Handler.log_path = log
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(server.server_address[1], flush=True)
    server.serve_forever()


def mask(v, d):
    if isinstance(v, dict):
        return {k: ("<id>" if k == "transfer_id" and isinstance(x, int)
                    else "<t>" if k == "time_transferring" else mask(x, d))
                for k, x in v.items()}
    if isinstance(v, (list, tuple)):
        return [mask(x, d) for x in v]
    if isinstance(v, str):
        return v.replace(d, "<D>")
    if hasattr(v, "value"):  # an Enum
        return v.value
    return v


def files(d):
    out = []
    for top, dirs, names in os.walk(d):
        dirs.sort()
        for n in sorted(names):
            p = os.path.join(top, n)
            with open(p, "rb") as f:
                data = f.read()
            out.append([os.path.relpath(p, d), len(data), hashlib.sha256(data).hexdigest()])
    return out


def take_log(log):
    with open(log, encoding="utf-8") as f:
        lines = [json.loads(line) for line in f if line.strip()]
    open(log, "w").close()
    return lines


def run(port, log, d):
    # pylint: disable=import-outside-toplevel
    from prusa.connect.printer.const import TransferType
    from prusa.connect.printer.download import DownloadMgr, Transfer

    base = f"http://127.0.0.1:{port}"
    usb = os.path.join(d, "usb")
    os.makedirs(usb)
    with open(os.path.join(usb, "file"), "wb") as f:
        f.write(b"a file where a folder is wanted")

    class Storage:
        def __init__(self, path_storage):
            self.path_storage = path_storage

    class Fs:
        sep = "/"
        storage_dict = {"usb": Storage(usb), "nopath": Storage(""),
                        "slash": Storage(usb + "/")}

    headers = {"Token": "secret", "User-Agent": "UA/1.0", "Fingerprint": "fp"}
    records = []

    def step(name, path, url=None, server=base, type_=TransferType.FROM_CONNECT,
             printed=None, stop_at_first_chunk=False, transfer=None, run_loop=True,
             again=False):
        events = []
        transfer = transfer or Transfer()
        mgr = DownloadMgr(Fs(), transfer, lambda: (server, dict(headers)),
                          lambda event, source, **kw: events.append(
                              [event.value, source.value, kw]),
                          lambda: printed, lambda t: events.append(["finished_cb", t.path]))
        if stop_at_first_chunk:
            transfer.progress_cb = lambda: transfer.transferred and transfer.stop()
        try:
            started = mgr.start(type_, path, url, to_print=True, to_select=False,
                                start_cmd_id=7, hash_="abc", team_id=3)
        except Exception as err:  # pylint: disable=broad-except
            started = [repr(err)]
        again_started = None
        if again:
            again_started = mgr.start(type_, path, url, start_cmd_id=8)
        if run_loop and transfer.in_progress:
            thread = threading.Thread(target=mgr.loop)
            thread.start()
            deadline = time.monotonic() + 30
            while transfer.in_progress and time.monotonic() < deadline:
                time.sleep(0.01)
            mgr.stop_loop()
            thread.join()
        records.append(mask({"step": name, "start": started, "again": again_started,
                             "events": events, "files": files(d),
                             "requests": take_log(log)}, d))
        return transfer

    # the name checks and to_os_path, without the loop
    for p in ["usb/a.gcode", "/usb", "/nope/a.gcode", "/nopath/a.gcode", "/usb/../x.gcode",
              "/usb/a#.gcode", "/usb/a?.gcode", "/usb/" + "é" * 125 + ".gcode",
              "/usb/" + "a" * 249, "/usb/" + "f" * 256 + "/a.gcode",
              "/usb/" + "é" * 255 + "/a.gcode", "/slash/s.gcode", "/usb//b.gcode",
              "/usb/./c.gcode"]:
        step("start " + p[:40], p, base + "/ok.gcode", run_loop=False)
    # downloads
    step("connect, token kept", "/usb/a.gcode", base + "/ok.gcode")
    step("another server: User-Agent only", "/usb/b.gcode", base + "/ok.gcode",
         server="http://example.invalid")
    step("no server", "/usb/b2.gcode", base + "/ok.gcode", server=None)
    step("server in upper case", "/usb/b3.gcode", base + "/ok.gcode",
         server=base.upper())
    step("text/plain", "/usb/c.gcode", base + "/plain.gcode")
    step("404", "/usb/d.gcode", base + "/missing")
    step("text/html", "/usb/e.gcode", base + "/html")
    step("a charset", "/usb/e2.gcode", base + "/charset")
    step("upper-case type", "/usb/e3.gcode", base + "/upper")
    step("no content type", "/usb/f.gcode", base + "/no-type")
    step("empty", "/usb/g.gcode", base + "/empty")
    step("chunked, no length", "/usb/h.gcode", base + "/chunked")
    step("gzip", "/usb/i.gcode", base + "/gzip")
    step("deflate", "/usb/j.gcode", base + "/deflate")
    step("into new folders", "/usb/sub/deeper/k.gcode", base + "/ok.gcode")
    step("into an existing folder", "/usb/sub/deeper/l.gcode", base + "/ok.gcode")
    step("a file where the folder is", "/usb/file/m.gcode", base + "/ok.gcode")
    step("stopped", "/usb/n.gcode", base + "/big", stop_at_first_chunk=True)
    step("the printed file", "/usb/a.gcode", base + "/ok.gcode",
         printed=os.path.join(usb, "a.gcode"))
    step("from the web", "/usb/o.gcode", base + "/ok.gcode", type_=TransferType.FROM_WEB)
    step("not a download type", "/usb/p.gcode", base + "/ok.gcode",
         type_=TransferType.FROM_CLIENT, run_loop=False)  # the loop leaves it alone
    step("another transfer in progress", "/usb/q.gcode", base + "/ok.gcode", again=True)
    step("unreachable", "/usb/r.gcode", "http://127.0.0.1:9/x")
    # a storage that is not there: the transfer runs on, with no os_path
    step("invalid storage, loop", "/nope/s.gcode", base + "/ok.gcode")
    # ... and with the os_path a transfer before it left
    t = step("then a good one", "/usb/t.gcode", base + "/ok.gcode")
    step("then an invalid storage again", "/nope/u.gcode", base + "/ok.gcode", transfer=t)
    print(json.dumps(records))


if __name__ == "__main__":
    if sys.argv[1] == "serve":
        serve(sys.argv[2])
    else:
        run(int(sys.argv[2]), sys.argv[3], sys.argv[4])
