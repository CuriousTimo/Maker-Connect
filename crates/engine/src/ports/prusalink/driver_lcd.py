from types import SimpleNamespace as NS

from prusa.connect.printer.conditions import (
    API, COND_TRACKER, HTTP, INTERNET, TOKEN, CondState)
from prusa.connect.printer.const import State, TransferType
from prusa.link.conditions import (
    DEVICE, FW, HW, ID, JOB_ID, LAN, PHY, SN, UPGRADED, use_connect_errors)
from prusa.link.printer_adapter import lcd_printer as lp
from prusa.link.printer_adapter.structures import carousel as cr
from prusa.link.printer_adapter.structures.carousel import LCDLine
from prusa.link.printer_adapter.structures.model_classes import JobState

CONDS = {"ID": ID, "FW": FW, "SN": SN, "UPGRADED": UPGRADED, "JOB_ID": JOB_ID,
         "HTTP": HTTP, "TOKEN": TOKEN, "API": API, "INTERNET": INTERNET,
         "LAN": LAN, "PHY": PHY, "DEVICE": DEVICE, "OTHER": HW}
use_connect_errors(True)

now = [0.0]
lp.time = lambda: now[0]
cr.time = lambda: now[0]
lp.prctl_name = lambda: None
lp.wait_for_instruction = lambda *args, **kwargs: True
sent = []
lp.enqueue_instruction = lambda queue, message, to_front=False, to_checksum=False: (
    sent.append([now[0], message, to_front]) or NS(message=message))


class FakeEvent:
    """`notiff_event`: `wait` is where the fake time passes and the script's actions happen"""

    def __init__(self):
        self.flag = False
        self.script = []
        self.end = 0.0
        self.apply = None

    def set(self):
        self.flag = True

    def clear(self):
        self.flag = False

    def is_set(self):
        return self.flag

    def wait(self, timeout=None):
        if not self.flag:
            now[0] += timeout
        while self.script and self.script[0]["at"] <= now[0]:
            self.apply(self.script.pop(0))
        if now[0] >= self.end:
            self.lcd.quit_evt.set()


def run(scenario):
    del sent[:]
    now[0] = 0.0
    lp.LCDPrinter._MCSingleton__instance = None
    model = NS(
        job=NS(job_state=JobState.IDLE, selected_file_path=None),
        file_printer=NS(recovering=False, recovery_ready=False),
        state_manager=NS(current_state=State.IDLE),
        ip_updater=NS(local_ip=None),
        latest_telemetry=NS(speed=None))
    settings = NS(is_wizard_needed=lambda: False,
                  printer=NS(network_error_chime=False))
    printer = NS(transfer=NS(in_progress=False, progress=0, type=TransferType.NO_TRANSFER,
                             transferred=0, size=0))
    lcd = lp.LCDPrinter(None, model, settings, printer, scenario["printer_number"])
    event = FakeEvent()
    event.lcd = lcd
    event.script = list(scenario["script"])
    event.end = scenario["end"]
    lcd.notiff_event = event

    def apply(action):
        for key, value in action.get("set", {}).items():
            if key == "job_in_progress":
                model.job.job_state = JobState.IN_PROGRESS if value else JobState.IDLE
            elif key == "selected_file_path":
                model.job.selected_file_path = value
            elif key in ("recovering", "recovery_ready"):
                setattr(model.file_printer, key, value)
            elif key == "state":
                model.state_manager.current_state = State[value]
            elif key == "wizard_needed":
                settings.is_wizard_needed = lambda value=value: value
            elif key == "network_error_chime":
                settings.printer.network_error_chime = value
            elif key == "local_ip":
                model.ip_updater.local_ip = value
            elif key == "speed":
                model.latest_telemetry.speed = value
            elif key == "transfer":
                printer.transfer = NS(in_progress=value["in_progress"], progress=value["progress"],
                                      type=TransferType[value["kind"]],
                                      transferred=value["transferred"], size=value["size"])
            elif key == "conditions":
                for cond in CONDS.values():
                    cond.state = CondState.OK
                if not value["lan"]:
                    LAN.state = CondState.NOK
                if value["error"] is not None:
                    CONDS[value["error"]].state = CondState.NOK
                worst = COND_TRACKER.get_worst()
                assert worst is CONDS.get(value["error"]), (worst, value)
            else:
                raise KeyError(key)
        if action.get("lcd_updated"):
            lcd.lcd_updated(object(), object())
        if "message" in action:
            lcd.print_message(LCDLine(action["message"]))
        if action.get("reset_error_grace"):
            lcd.reset_error_grace()

    event.apply = apply
    lcd._lcd_printer()
    return sent[:]


print(json.dumps([run(scenario) for scenario in json.load(sys.stdin)]))
