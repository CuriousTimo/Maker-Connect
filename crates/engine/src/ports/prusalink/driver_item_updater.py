from prusa.link.printer_adapter.structures import item_updater as module
from prusa.link.printer_adapter.structures.item_updater import (
    ItemUpdater,
    SideEffectOnly,
    WatchedGroup,
    WatchedItem,
)

# PrusaLink's own ItemUpdater through a list of operations per case, its `time` patched to a clock
# the case sets, its threads not started: a "turn" runs one round of a thread's own loop (the
# queue's `get` ends the loop after that round; both events are set first, so a wait returns at
# once). Items gather from a script of values, errors and side effects; every signal and write is
# logged. After each operation: the error, if any, the log since the last one, and the state of
# the items, the groups and the queues (floats as repr()).
request = json.load(sys.stdin)
SIGNALS = ("became_valid_signal", "became_invalid_signal", "timed_out_signal",
           "error_refreshing_signal", "validation_error_signal", "value_changed_signal",
           "val_err_timeout_signal")
LOOPS = {"refresher": ("refresh_queue", "_refresher"),
         "invalidations": ("invalidate_timers", "_process_invalidations"),
         "timeouts": ("timeout_timers", "_process_timeouts")}


def error(exc):
    # a KeyError's text is the item's repr, its address
    return [type(exc).__name__, "" if isinstance(exc, KeyError) else str(exc)]


def run_case(ops):
    clock = [1000.0]
    module.time = lambda: clock[0]
    updater = ItemUpdater(quit_interval=0.0)
    items, groups, log, keep = {}, {}, [], []

    def act(action):
        kind = action[0]
        if kind == "invalidate_group":
            updater.invalidate_group(groups[action[1]])
        elif kind == "invalidate":
            updater.invalidate(items[action[1]])
        elif kind == "set_value":
            updater.set_value(items[action[1]], action[2])

    def gatherer(name, script):
        steps = iter(script)

        def gather():
            step = next(steps)
            log.append(["gather", name])
            if isinstance(step, dict):
                if "raise" in step:
                    raise RuntimeError(step["raise"])
                if "set" in step:
                    updater.set_value(items[name], step["set"])
                raise SideEffectOnly()
            return step
        return gather

    def validator(kind):
        def validate(value):
            if kind == "only_42":
                return value == 42
            if value == 13:
                raise RuntimeError("thirteen")
            return True
        return validate

    def make_item(name, args):
        kwargs = {}
        if args.get("gather") is not None:
            kwargs["gather_function"] = gatherer(name, args["gather"])
        if args.get("validation") is not None:
            kwargs["validation_function"] = validator(args["validation"])
        kwargs["write_function"] = lambda value: log.append(["write", name, value])
        for key in ("interval", "timeout", "on_fail_interval"):
            if key in args:
                kwargs[key] = args[key]
        item = WatchedItem(name, **kwargs)
        for signal in SIGNALS:
            def receiver(sender, signal=signal):
                log.append([signal, sender if signal == "value_changed_signal"
                            else sender.name])
            keep.append(receiver)
            getattr(item, signal).connect(receiver)
        items[name] = item

    def make_group(name, members):
        group = WatchedGroup([items[m] for m in members])
        for signal in ("became_valid_signal", "became_invalid_signal"):
            def receiver(sender, signal=signal, name=name):
                log.append(["group " + signal, name])
            keep.append(receiver)
            getattr(group, signal).connect(receiver)
        groups[name] = group

    def turn(loop):
        queue_name, method = LOOPS[loop]
        queue = getattr(updater, queue_name)
        original = queue.get

        def get(*args, **kwargs):
            updater.running = False
            return original(*args, **kwargs)
        queue.get = get
        updater.running = True
        updater.invalidate_queue_event.set()
        updater.timeout_queue_event.set()
        try:
            getattr(updater, method)()
        finally:
            del queue.get
            updater.running = True

    def state():
        def num(x):
            # the default on_fail_interval is the int 5: compared as the float it adds up to
            return "None" if x is None else repr(float(x))

        def item_state(i):
            return [i.valid, i.scheduled, num(i.interval), i.disabled, num(i.on_fail_interval),
                    num(i.timeout), num(i.invalidate_at), num(i.times_out_at), i.value]
        return {
            "items": {n: item_state(i) for n, i in items.items()},
            "groups": {n: [g.valid, sorted(i.name for i in g.valid_items),
                           sorted(i.name for i in g.invalid_items)]
                       for n, g in groups.items()},
            "tracked": sorted(i.name for i in updater.items),
            "refresh": [i.name for i in updater.refresh_queue.queue],
            "invalidate_timers": [[repr(t), i.name] for t, i in updater.invalidate_timers.queue],
            "timeout_timers": [[repr(t), i.name] for t, i in updater.timeout_timers.queue],
        }

    steps = []
    for op in ops:
        kind = op[0]
        log.clear()
        try:
            if kind == "item":
                make_item(op[1], op[2])
            elif kind == "group":
                make_group(op[1], op[2])
            elif kind == "on":
                def receiver(sender, action=op[3]):
                    act(action)
                keep.append(receiver)
                getattr(items[op[1]], op[2]).connect(receiver)
            elif kind == "clock":
                clock[0] = op[1]
            elif kind == "add":
                updater.add_item(items[op[1]], start_tracking=op[2])
            elif kind == "invalidate":
                updater.invalidate(items[op[1]])
            elif kind == "invalidate_group":
                updater.invalidate_group(groups[op[1]])
            elif kind == "disable":
                updater.disable(items[op[1]])
            elif kind == "enable":
                updater.enable(items[op[1]])
            elif kind == "set_value":
                updater.set_value(items[op[1]], op[2])
            elif kind == "schedule":
                updater.schedule_invalidation(items[op[1]], interval=op[2], reschedule=op[3])
            elif kind == "cancel":
                updater.cancel_scheduled_invalidation(items[op[1]])
            elif kind == "set_interval":
                items[op[1]].interval = op[2]
            elif kind == "time_out":
                updater._time_out(items[op[1]])  # pylint: disable=protected-access
            elif kind == "turn":
                turn(op[1])
            result = None
        except Exception as exc:  # pylint: disable=broad-except
            result = error(exc)
        steps.append([result, list(log), state()])
    return steps


print(json.dumps([run_case(ops) for ops in request["cases"]]))
