from prusa.link.printer_adapter.structures.mc_singleton import MCSingleton


# get_instance before, the first call, get_instance after, a second call: what each gives.
class Thing(metaclass=MCSingleton):
    def __init__(self, value):
        self.value = value


def attempt(value):
    try:
        return {"ok": Thing(value).value}
    except Exception as error:  # pylint: disable=broad-except
        return {"error": [type(error).__name__, str(error)]}


def instance():
    found = Thing.get_instance()
    return None if found is None else found.value


request = json.load(sys.stdin)
print(json.dumps([instance(), attempt(1), instance(), attempt(2), instance()]))
