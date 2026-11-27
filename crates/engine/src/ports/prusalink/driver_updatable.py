from threading import Event

from prusa.link.printer_adapter.updatable import ThreadedUpdatable
from prusa.link.util import loop_until

# loop_until: what is called in which order until the event is set by the third run; a
# ThreadedUpdatable whose update stops it at the third: its calls, and that it has stopped.
request = json.load(sys.stdin)
order = []
evt = Event()


def every():
    order.append("every")
    return 0.0


def to_run():
    order.append("run")
    if order.count("run") == 3:
        evt.set()


loop_until(evt, every, to_run)


class Counter(ThreadedUpdatable):
    thread_name = "counter"
    update_interval = 0.001

    def __init__(self):
        self.calls = 0
        super().__init__()

    def update(self):
        self.calls += 1
        if self.calls == 3:
            self.stop()


counter = Counter()
counter.start()
counter.wait_stopped()
try:
    ThreadedUpdatable().update()
    base = None
except NotImplementedError as error:
    base = type(error).__name__
print(json.dumps({"order": order, "calls": counter.calls,
                  "alive": counter.thread.is_alive(),
                  "name": [ThreadedUpdatable.thread_name, counter.thread.name],
                  "interval": ThreadedUpdatable.update_interval, "base": base}))
