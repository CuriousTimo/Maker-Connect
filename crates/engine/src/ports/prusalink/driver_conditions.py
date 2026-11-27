import ast

from prusa.connect.printer import conditions as sdk_conditions
from prusa.connect.printer.conditions import CondState, Condition
from prusa.link import conditions
from prusa.link.config import Settings

# "errors": every class of the module after LinkError, in the source's order (from the AST), with
# LinkError itself first: its name, its base's, the class attributes as Python resolves them, and
# an instance made without details and with: details, path, template, args, url, gen_headers(),
# and gen_headers() once its url is set; or the exception. "tree": the conditions as imported, then
# each op (set a condition's state, use_connect_errors, a settings file read as Settings.instance)
# with its result or exception, and after it status() (sorted by name), printer_status() and
# connect_status(), and the SDK's COND_TRACKER's worst.
# "second": the SDK's INTERNET joined to a second Lan.
request = json.load(sys.stdin)
source = open(conditions.__file__).read()
names = ["LinkError"] + [n.name for n in ast.parse(source).body
                         if isinstance(n, ast.ClassDef) and n.name != "LinkError"]


def instance(cls, details):
    try:
        error = cls(details) if details is not None else cls()
    except Exception as e:  # pylint: disable=broad-except
        return [type(e).__name__, str(e)]
    out = [error.details, error.path, error.template, list(error.args), error.url,
           error.gen_headers()]
    error.url = "http://link/error/x"
    out.append(error.gen_headers())
    return out


errors = []
for name in names:
    cls = getattr(conditions, name)
    errors.append([name, cls.__bases__[0].__name__, getattr(cls, "title", None),
                   getattr(cls, "text", None), cls.id, getattr(cls, "status_code", None),
                   cls.use_basic_template, instance(cls, None), instance(cls, "it is gone")])


def snapshot():
    # sorted: the children are a set (the SDK's conditions.py:49), their order undefined
    return [sorted([k, list(v)] for k, v in conditions.status().items()),
            conditions.printer_status(), conditions.connect_status(),
            getattr(sdk_conditions.COND_TRACKER.get_worst(), "name", None)]


Settings(request["settings"][0])
tree = [snapshot()]
for op in request["ops"]:
    try:
        if op[0] == "set":
            target = getattr(conditions if hasattr(conditions, op[1]) else sdk_conditions, op[1])
            target.state = CondState[op[2]]
        elif op[0] == "use_connect":
            conditions.use_connect_errors(op[1])
        else:
            # one instance (config.py:273-274): reset, as PrusaLink.s tests reset singletons
            Settings.instance = None
            Settings(request["settings"][op[1]])
        result = None
    except Exception as e:  # pylint: disable=broad-except
        result = [type(e).__name__, str(e)]
    tree.append([result] + snapshot())
try:
    conditions.INTERNET.set_parent(Condition("Lan", "Eth|WLAN has no IP address"))
    second = None
except Exception as e:  # pylint: disable=broad-except
    second = [type(e).__name__, str(e)]
print(json.dumps({"errors": errors, "tree": tree, "second": second}))
