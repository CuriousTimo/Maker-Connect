import os
import re
import tempfile

from prusa.connect.printer.conditions import Condition, CondState
from prusa.link.serial import serial_queue as module
from prusa.link.serial.instruction import (
    Instruction,
    MandatoryMatchableInstruction,
    MatchableInstruction,
)
from prusa.link.serial.serial_parser import SerialParser
from prusa.link.serial.serial_queue import MonitoredSerialQueue, SerialQueue

# PrusaLink's own SerialQueue and MonitoredSerialQueue through a list of operations per case: the
# real SerialParser (lines are fed to its `decide`), the real IsPlannerFed on a file of the case,
# a serial adapter that logs what is written and closed, fresh `SERIAL` and `RPI_ENABLED`
# conditions, the module's `time` set by the case and its `Thread` one that logs and starts
# nothing; `InterestingLogRotator.trigger` (logging) does nothing. After each operation: the error,
# if any, the log since the last one, and the state.
request = json.load(sys.stdin)


def error(exc):
    return [type(exc).__name__, str(exc)]


def data(value):
    return None if value is None else value.decode("latin-1")


def run_case(case):
    log = []
    clock = [1000.25]
    module.time = lambda: clock[0]

    class Thread:
        def __init__(self, target=None, args=(), name=None, daemon=None):
            self.name = {"_keep_sending": "KeepSending",
                         "keep_monitoring": "KeepMonitoring",
                         "_printer_reconnected": "PrinterReconnected"}[target.__name__]
            log.append(["thread", self.name, list(args), bool(daemon)])

        def start(self):
            log.append(["start", self.name])

        def join(self):
            log.append(["join", self.name])

    module.Thread = Thread

    class InterestingLogRotator:  # the rotation of the log files: not covered, as the tests patch it
        @staticmethod
        def trigger(reason):
            del reason

    module.InterestingLogRotator = InterestingLogRotator

    class Adapter:
        def write(self, value):
            log.append(["write", data(value)])

        def close(self):
            log.append(["close"])

    serial = Condition("Port", "Serial device does not exist", priority=570)
    rpi = Condition("RPIenabled", "RPi port is not enabled", parent=serial, priority=560)
    module.SERIAL, module.RPI_ENABLED = serial, rpi

    folder = tempfile.mkdtemp()
    path = os.path.join(folder, "threshold")
    if case["threshold"] is not None:
        with open(path, "w", encoding="utf-8") as file:
            file.write(case["threshold"])

    SerialParser._MCSingleton__instance = None
    SerialQueue._MCSingleton__instance = None
    MonitoredSerialQueue._MCSingleton__instance = None
    parser = SerialParser()
    kind = MonitoredSerialQueue if case["monitored"] else SerialQueue
    queue = kind(Adapter(), parser, path)
    queue.serial_queue_failed.connect(
        lambda sender: log.append(["failed"]), weak=False)
    queue.instruction_confirmed_signal.connect(
        lambda sender: log.append(["confirmed"]), weak=False)
    queue.message_number_changed.connect(
        lambda number: log.append(["number_changed", number]), weak=False)
    made = []

    def make(spec):
        kind, message, to_checksum = spec[0], spec[1], spec[2]
        if kind == "plain":
            instruction = Instruction(message, to_checksum=to_checksum)
        else:
            cls = MatchableInstruction if kind == "matchable" else MandatoryMatchableInstruction
            instruction = cls(message, to_checksum=to_checksum,
                              capture_matching=re.compile(spec[3]))
        made.append(instruction)
        return instruction

    def instr(instruction):
        if instruction is None:
            return None
        return [instruction.message, data(instruction.data), instruction.number,
                instruction.is_sent(), instruction.is_confirmed(),
                len(instruction.get_matches()) if hasattr(instruction, "get_matches") else None]

    def state():
        return {
            "queue": [instr(i) for i in queue.queue],
            "priority_queue": [instr(i) for i in queue.priority_queue],
            "current": instr(queue.current_instruction),
            "message_number": queue.message_number,
            "send_history": [instr(i) for i in queue.send_history],
            "recovery_list": [instr(i) for i in queue.recovery_list],
            "rx_yeet_slot": instr(queue.rx_yeet_slot),
            "m110_workaround_slot": instr(queue.m110_workaround_slot),
            "worked_around_m110": queue.worked_around_m110,
            "closed": queue.closed,
            "has_failed": queue.has_failed,
            "block_sending": queue._block_sending,
            "send_event": queue.send_event.is_set(),
            "quit_evt": queue.quit_evt.is_set(),
            "is_fed": queue.is_planner_fed(),
            "planner_items": queue.is_planner_fed.item_count,
            "stuck_counter": getattr(queue, "stuck_counter", None),
            "last_event_on": repr(getattr(queue, "last_event_on", None)),
            "rpi": rpi.state.name,
            "can_write": queue.can_write(),
            "is_empty": queue.is_empty(),
            "peek_next": instr(queue.peek_next()),
            "delay": repr(float(queue.get_current_delay())) if case["monitored"] else None,
            "pairings": [str(p) for p in parser.pattern_list],
        }

    steps = [[None, list(log), state()]]
    for op in case["ops"]:
        kind = op[0]
        log.clear()
        try:
            if kind == "enqueue":
                queue.enqueue_one(make(op[1]), to_front=op[2])
            elif kind == "enqueue_list":
                queue.enqueue_list([make(spec) for spec in op[1]], to_front=op[2])
            elif kind == "send":
                queue._send()
            elif kind == "line":
                parser.decide(op[1])
            elif kind == "confirm":
                queue._confirmed(force=op[1])
            elif kind == "resend":
                queue._resend(op[1])
            elif kind == "yeet":
                queue._rx_got_yeeted()
            elif kind == "flush_print":
                queue.flush_print_queue()
            elif kind == "flush_queues":
                queue._flush_queues()
            elif kind == "reset_number":
                queue.reset_message_number()
            elif kind == "set_number":
                queue.set_message_number(op[1])
            elif kind == "message_number":
                queue.message_number = op[1]
            elif kind == "replenish":
                queue.replenish_history(op[1])
            elif kind == "block":
                queue.block_sending()
            elif kind == "unblock":
                queue.unblock_sending()
            elif kind == "clear_send_event":
                queue.send_event.clear()
            elif kind == "close":
                queue.closed = True
            elif kind == "has_failed":
                queue.has_failed = True
            elif kind == "rx_max":
                queue.rx_max = op[1]
            elif kind == "reconnected":
                queue._printer_reconnected(op[1], op[2])
            elif kind == "printer_reconnected":
                queue.printer_reconnected(op[1], op[2])
            elif kind == "clock":
                clock[0] = op[1]
            elif kind == "serial":
                serial.state = CondState[op[1]]
            elif kind == "check_status":
                queue.check_status()
            elif kind == "stop":
                queue.stop()
            elif kind == "wait_stopped":
                queue.wait_stopped()
            result = None
        except Exception as exc:  # pylint: disable=broad-except
            result = error(exc)
        steps.append([result, list(log), state()])
    return steps


print(json.dumps([run_case(case) for case in request["cases"]]))
