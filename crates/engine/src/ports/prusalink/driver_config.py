# PrusaLink's `config.py` (`Config`, `Settings`) on the examples of `config.rs`'s test: per case
# the models it builds (each value's type and `str()`), what `update_sections` then `write` give,
# the log handler's kind and format, and an exception's class and `str()`.
import io
import os
import tempfile

from prusa.link import config as link_config
from prusa.link.config import Config, FakeArgs, Settings


def model(m):
    return [[k, type(v).__name__, str(v)] for k, v in m.items()]


def error(err):
    return {"error": [type(err).__name__, str(err)]}


def written(parser):
    out = io.StringIO()
    parser.write(out)
    return out.getvalue()


def run_config(case, d):
    path = os.path.join(d, "prusalink.ini")
    if case.get("ini") is not None:
        with open(path, "w", encoding="utf-8") as f:
            f.write(case["ini"])
    args = FakeArgs(path)
    for key, value in (case.get("args") or {}).items():
        setattr(args, key, value)
    try:
        cfg = Config(args)
        handler = cfg.configured_handler
        kind = type(handler).__name__
        address = getattr(handler, "address", None)
        fmt = handler.formatter._fmt
        result = {
            "debug": cfg.debug,
            "daemon": model(cfg.daemon),
            "log_settings": [[k, v] for k, v in cfg.log_settings.items()],
            "http": model(cfg.http),
            "printer": model(cfg.printer),
            "cameras": model(cfg.cameras),
            "handler": [kind, None if address is None else str(address), fmt],
        }
        try:
            cfg.update_sections()
            result["written"] = written(cfg)
        except Exception as err:  # noqa: BLE001
            result["written_error"] = [type(err).__name__, str(err)]
        return result
    except Exception as err:  # noqa: BLE001
        return error(err)


def run_settings(case, d):
    path = os.path.join(d, "prusa_printer_settings.ini")
    if case.get("settings") is not None:
        with open(path, "w", encoding="utf-8") as f:
            f.write(case["settings"])
    Settings.instance = None
    try:
        s = Settings(path)
        result = {
            "printer": model(s.printer),
            "network": model(s.network),
            "service_connect": model(s.service_connect),
            "service_local": model(s.service_local),
            "is_wizard_needed": s.is_wizard_needed(),
            "use_connect": s.use_connect(),
        }
    except Exception as err:  # noqa: BLE001
        result = error(err)
    result["file"] = open(path, encoding="utf-8").read() if os.path.exists(path) else None
    Settings.instance = None
    return result


out = []
for case in json.load(sys.stdin):
    with tempfile.TemporaryDirectory() as d:
        result = run_settings(case, d) if "settings" in case else run_config(case, d)
        # the temporary folder, named in a parsing error, as DIR
        out.append(json.loads(json.dumps(result).replace(json.dumps(d)[1:-1], "DIR")))
print(json.dumps({"cases": out, "log_foreground": link_config.LOG_FORMAT_FOREGROUND}))
