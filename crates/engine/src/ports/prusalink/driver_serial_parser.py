import re

from prusa.link.serial.serial_parser import SerialParser

# Each op: add (pattern, handler, priority), remove (pattern, handler), decide (line). After each:
# the result (None, or the exception's class and text; for decide the handlers called, sorted, with
# the match's groups) and the pairings' str in pattern_list's order.
request = json.load(sys.stdin)
patterns = [re.compile(p) for p in request["patterns"]]
parser = SerialParser()
calls = []


def make(i):
    def handler(sender, match):
        calls.append([i, [match.group(0)] + list(match.groups())])
    return handler


handlers = [make(i) for i in range(8)]
results = []
for op in request["ops"]:
    try:
        if op[0] == "add":
            priority = float("inf") if op[3] == "inf" else op[3]
            if isinstance(priority, float) and priority.is_integer():
                priority = int(priority)
            parser.add_handler(patterns[op[1]], handlers[op[2]], priority)
            result = None
        elif op[0] == "remove":
            parser.remove_handler(patterns[op[1]], handlers[op[2]])
            result = None
        else:
            calls.clear()
            parser.decide(op[1])
            result = sorted(calls)
    except Exception as error:  # pylint: disable=broad-except
        result = [type(error).__name__, str(error)]
    results.append([result, [str(p) for p in parser.pattern_list]])
from prusa.link.serial.serial_parser import RegexPairing

strs = []
for value in [0, 1, 2.5, float("inf"), 1764270000.25, 2]:
    pairing = RegexPairing(patterns[1], priority=value)
    strs.append(str(pairing))
    for _ in range(2):
        pairing.signal.connect(lambda *_, **__: None, weak=False)
        strs.append(str(pairing))
print(json.dumps({"results": results, "strs": strs}))
