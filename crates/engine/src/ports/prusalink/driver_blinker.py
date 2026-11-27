from blinker import Signal

# Each op: connect (weak or strong), disconnect, drop (the driver's own reference, so a weak
# receiver is collected), send (the calls, sorted: the order is undefined), len(receivers).
# A receiver "selfdis*" disconnects itself when called, "adder*" connects "late" (strong).
request = json.load(sys.stdin)
signal = Signal()
calls = []
held = {}


def make(name):
    def receiver(sender):
        calls.append([name, sender])
        if name.startswith("selfdis"):
            signal.disconnect(held_self[name])
        if name.startswith("adder"):
            signal.connect(late, weak=False)
    return receiver


held_self = {}
late = make("late")
for name in request["names"]:
    held[name] = make(name)
    held_self[name] = held[name]

results = []
for op in request["ops"]:
    kind = op[0]
    if kind == "connect":
        signal.connect(held[op[1]], weak=op[2])
        results.append(None)
    elif kind == "disconnect":
        signal.disconnect(held[op[1]])
        results.append(None)
    elif kind == "drop":
        del held[op[1]]
        del held_self[op[1]]
        results.append(None)
    elif kind == "send":
        calls.clear()
        signal.send(op[1])
        results.append(sorted(calls))
    elif kind == "len":
        results.append([len(signal.receivers), bool(signal.receivers)])
print(json.dumps(results))
