import collections
import threading
from types import SimpleNamespace as NS

from prusa.link.printer_adapter import file_printer
from prusa.link.printer_adapter.file_printer import FilePrinter
from prusa.link.printer_adapter.print_stats import PrintStats
from prusa.link.util import get_gcode

enqueued = []


def enqueue_instruction(queue, message, **kwargs):
    enqueued.append([message, kwargs.get("to_front"), bool(kwargs.get("to_checksum"))])
    return NS(message=message)


file_printer.enqueue_instruction = enqueue_instruction


def stand_in(model, stats):
    """What `FilePrinter` is to its own methods, with `print_stats.get_stats` given"""
    me = NS(lock=threading.RLock(),
            data=NS(gcode_number=0, enqueued=collections.deque()),
            model=model,
            print_stats=NS(get_time_printing=lambda: 0.0, get_stats=stats),
            serial_queue=None,
            time_printing_signal=NS(send=lambda *args, **kwargs: None))
    me.to_print_stats = lambda number: FilePrinter.to_print_stats(me, number)
    me.send_print_stats = lambda: FilePrinter.send_print_stats(me)
    return me


request = json.load(sys.stdin)
files = []
for path in request["files"]:
    del enqueued[:]
    model = NS()
    track = PrintStats(model)
    track.track_new_print(path)
    me = stand_in(model, lambda number: ((number * 7) % 101, (number * 13) % 977))
    with open(path, encoding='utf-8') as gcode_file:
        for line in gcode_file:
            gcode = get_gcode(line)
            if gcode:
                FilePrinter.print_gcode(me, gcode)
    files.append({"total": model.print_stats.total_gcode_count,
                  "inbuilt": model.print_stats.has_inbuilt_stats,
                  "enqueued": list(enqueued)})

rounding = []
for completion, seconds in request["rounding"]:
    del enqueued[:]
    # `print_stats.py:98-103`: `round(sec_remaining / 60)` and `round(fraction_done * 100)`
    me = stand_in(NS(), lambda number: (round(completion), round(seconds / 60)))
    me.data.gcode_number = 1
    FilePrinter.send_print_stats(me)
    rounding.append(enqueued[0][0])
print(json.dumps({"files": files, "rounding": rounding}))
