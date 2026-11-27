import os

from prusa.link.serial import is_planner_fed as module
from prusa.link.serial.is_planner_fed import IsPlannerFed, TimeValue

# PrusaLink's own IsPlannerFed through a list of operations, each case in its own folder under
# `base`: make the planner (with the module's constants patched, as PrusaLink's tests do), write a
# threshold file, process a value (or many: is_fed and the threshold after each), save, balance,
# push to a heap, remove the oldest. After each: the error, if any, and the planner's state
# (floats as repr()). `base` reads BASE in the output.
request = json.load(sys.stdin)
base = request["base"]
CONSTS = {name: getattr(module, name) for name in
          ("QUEUE_SIZE", "HEAP_RATIO", "IGNORE_ABOVE", "DEFAULT_THRESHOLD",
           "USE_DYNAMIC_THRESHOLD")}


def text(value):
    return str(value).replace(base, "BASE")


def heap(h):
    return [[repr(i.value), repr(i.heap_value), i.heap_index,
             i.heap_name.value if i.heap_name else None] for i in h.heap]


def attempt(call):
    try:
        return repr(call())
    except Exception as error:  # pylint: disable=broad-except
        return [type(error).__name__, text(error)]


def state(planner, path):
    if planner is None:
        return None
    contents = None
    if os.path.isfile(path):
        with open(path, "rb") as file:
            contents = list(file.read())
    return {
        "is_fed": planner(),
        "default_threshold": repr(planner.default_threshold),
        "item_count": planner.item_count,
        "threshold": attempt(lambda: planner.threshold),
        "dynamic": attempt(planner.get_dynamic_threshold),
        "short": heap(planner.short_times),
        "long": heap(planner.long_times),
        "queue": [[repr(i.value), i.heap_name.value if i.heap_name else None]
                  for i in planner.times_queue],
        "file": contents,
        "path": text(planner.threshold_path),
    }


cases = []
for number, ops in enumerate(request["cases"]):
    folder = os.path.join(base, str(number))
    os.makedirs(folder)
    planner, path, steps = None, None, []
    for op in ops:
        kind = op[0]
        try:
            if kind == "new":
                for name, value in CONSTS.items():
                    setattr(module, name, value)
                for name, value in (op[2] or {}).items():
                    setattr(module, name, value)
                path = op[1] if op[1].startswith("raw:") else os.path.join(folder, op[1])
                path = path.removeprefix("raw:")
                planner = None
                planner = IsPlannerFed(path)
            elif kind == "write":
                full = os.path.join(folder, op[1])
                os.makedirs(os.path.dirname(full), exist_ok=True)
                with open(full, "wb") as file:
                    file.write(bytes(op[2]))
            elif kind == "mkdir":
                os.makedirs(os.path.join(folder, op[1]))
            elif kind == "process":
                planner.process_value(op[1])
            elif kind == "process_many":
                # many values, one state at the end: per value is_fed and the threshold
                seen = []
                for value in op[1]:
                    planner.process_value(value)
                    seen.append([planner(), repr(planner.threshold)])
            elif kind == "save":
                planner.save()
            elif kind == "balance":
                planner.balance()
            elif kind == "short_push":
                planner._short_push(TimeValue(op[1]))  # pylint: disable=protected-access
            elif kind == "long_push":
                planner._long_push(TimeValue(op[1]))  # pylint: disable=protected-access
            elif kind == "remove_last":
                planner._remove_last()  # pylint: disable=protected-access
            result = seen if kind == "process_many" else None
        except Exception as error:  # pylint: disable=broad-except
            result = [type(error).__name__, text(error)]
        steps.append([result, state(planner, planner.threshold_path if planner else "")])
    cases.append(steps)
print(json.dumps(cases))
