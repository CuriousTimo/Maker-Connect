from prusa.link.printer_adapter.structures import model_classes as m

# Each case: a model made with the given fields (a nested `slot` and its `slots` made as their own
# models), then its `dict(**kwargs)`.
request = json.load(sys.stdin)


def make(name, fields):
    fields = dict(fields)
    if name == "Telemetry" and fields.get("slot") is not None:
        fields["slot"] = make("Slot", fields["slot"])
    if name == "Slot" and fields.get("slots") is not None:
        fields["slots"] = {key: make("IndividualSlot", value)
                           for key, value in fields["slots"]}
    return getattr(m, name)(**fields)


print(json.dumps([make(case["model"], case["fields"]).dict(**case["kwargs"])
                  for case in request["cases"]]))
