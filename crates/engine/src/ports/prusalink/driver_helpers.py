import re
from threading import Event

from prusa.link.serial.helpers import (
    enqueue_instruction,
    enqueue_list_from_str,
    enqueue_matchable,
    wait_for_instruction,
)

# Each call: ["instruction", message, to_front, to_checksum], ["matchable", message, pattern,
# to_front, to_checksum, has_to_match] or ["list", messages, pattern, to_front, to_checksum]. The
# result: what the queue was given ([method, to_front, instructions]) and what the call returned
# (or the exception's class and text), and whether those are the same objects. An instruction as
# its class, str, to_checksum, capture_matching and capturing_regexps. Each wait: [answers of
# wait_for_confirmation (then True), how often should_wait is true (None: always), the event set]:
# the result and how often wait_for_confirmation was called.


class Queue:
    def __init__(self):
        self.calls = []

    def enqueue_one(self, instruction, to_front=False):
        self.calls.append(["one", to_front, [instruction]])

    def enqueue_list(self, instruction_list, to_front=False):
        self.calls.append(["list", to_front, list(instruction_list)])


def describe(instruction):
    capture = getattr(instruction, "capture_matching", None)
    return [type(instruction).__name__, str(instruction), instruction.to_checksum,
            None if capture is None else capture.pattern,
            [p.pattern for p in instruction.capturing_regexps]]


class Waiting:
    def __init__(self, answers):
        self.answers = list(answers)
        self.calls = 0

    def wait_for_confirmation(self, timeout=None):
        self.calls += 1
        return self.answers.pop(0) if self.answers else True


request = json.load(sys.stdin)
calls = []
for call in request["calls"]:
    queue = Queue()
    try:
        if call[0] == "instruction":
            returned = enqueue_instruction(queue, call[1], to_front=call[2], to_checksum=call[3])
        elif call[0] == "matchable":
            returned = enqueue_matchable(queue, call[1], re.compile(call[2]), to_front=call[3],
                                         to_checksum=call[4], has_to_match=call[5])
        else:
            returned = enqueue_list_from_str(queue, call[1], re.compile(call[2]),
                                             to_front=call[3], to_checksum=call[4])
        given = [i for c in queue.calls for i in c[2]]
        objects = returned if isinstance(returned, list) else [returned]
        same = len(given) == len(objects) and all(a is b for a, b in zip(given, objects))
        out = [describe(i) for i in objects] if isinstance(returned, list) \
            else describe(returned)
    except Exception as error:  # pylint: disable=broad-except
        out, same = [type(error).__name__, str(error)], None
    queued = [[c[0], c[1], [describe(i) for i in c[2]]] for c in queue.calls]
    calls.append([queued, out, same])
waits = []
for answers, times, evt_set in request["waits"]:
    instruction = Waiting(answers)
    left = [times]

    def should_wait():
        if left[0] is None:
            return True
        left[0] -= 1
        return left[0] >= 0

    event = Event()
    if evt_set:
        event.set()
    result = wait_for_instruction(instruction, should_wait, event, check_every=0)
    waits.append([result, instruction.calls])
print(json.dumps({"calls": calls, "waits": waits}))
