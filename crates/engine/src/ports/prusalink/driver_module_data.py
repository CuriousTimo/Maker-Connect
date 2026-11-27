from collections import deque
from enum import Enum

from prusa.link.printer_adapter.structures import module_data_classes as m

# Each case: [class, kwargs]: the model made from them, its dict() and dict(exclude_none=True)
# (an Enum as its value, a set sorted, a deque a list), Port's str, JobData's
# get_job_id_for_api().
request = json.load(sys.stdin)


def plain(value):
    if isinstance(value, Enum):
        return value.value
    if isinstance(value, (set, frozenset)):
        return sorted(value)
    if isinstance(value, (list, tuple, deque)):
        return [plain(item) for item in value]
    if isinstance(value, dict):
        return {key: plain(item) for key, item in value.items()}
    return value


results = []
for name, kwargs in request["cases"]:
    model = getattr(m, name)(**kwargs)
    out = [plain(model.dict()), plain(model.dict(exclude_none=True))]
    if name == "Port":
        out.append(str(model))
    if name == "JobData":
        out.append(model.get_job_id_for_api())
    results.append(out)
print(json.dumps(results))
