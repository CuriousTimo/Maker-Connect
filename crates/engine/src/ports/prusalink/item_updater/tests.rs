use super::*;
use crate::ports::octoprint::comm::py_float_repr;
use crate::ports::prusalink::python;
use serde_json::{Value, json};
use std::collections::HashMap;

type V = Option<i64>;
type Log = Arc<Mutex<Vec<Value>>>;

/// The cases (see `driver_item_updater.py`); the first ones follow PrusaLink's own tests
/// (`tests/test_item_updater.py`) with the clock in place of sleeping. A case starting with
/// `["sorted"]` has an item in two groups: the order of their handlers is a set's (difference 3),
/// so its logs are compared sorted.
fn cases() -> Vec<Value> {
    vec![
        // test_basics
        json!([["item", "basic_item", {"gather": [42, 42]}], ["add", "basic_item", true],
               ["turn", "refresher"], ["invalidate", "basic_item"], ["turn", "refresher"]]),
        // test_group
        json!([["item", "item_1", {"gather": [1, 1]}], ["item", "item_2", {"gather": [2, 2]}],
               ["group", "g", ["item_1", "item_2"]], ["add", "item_1", true],
               ["add", "item_2", true], ["turn", "refresher"], ["turn", "refresher"],
               ["invalidate", "item_1"], ["invalidate", "item_2"], ["turn", "refresher"],
               ["turn", "refresher"]]),
        // test_scheduled_invalidation
        serde_json::from_str::<Value>(
            r#"[["item", "item_1", {"gather": [null, null]}],
               ["item", "item_2", {"gather": [null, null]}],
               ["item", "item_3", {"gather": [null, null]}],
               ["item", "item_4", {"gather": [null, null]}],
               ["item", "item_5", {"gather": [null, null]}],
               ["item", "item_7", {"gather": [null, null], "interval": 0.2}],
               ["item", "item_8", {"gather": [null, null], "interval": 0.2}],
               ["item", "item_9", {"gather": [null, null], "interval": 0.2}],
               ["group", "g", ["item_1", "item_2", "item_3", "item_4", "item_5", "item_7",
                               "item_8", "item_9"]],
               ["add", "item_1", true], ["add", "item_2", true], ["add", "item_3", true],
               ["add", "item_4", true], ["add", "item_5", true], ["add", "item_7", true],
               ["add", "item_8", true], ["add", "item_9", true],
               ["turn", "refresher"], ["turn", "refresher"], ["turn", "refresher"],
               ["turn", "refresher"], ["turn", "refresher"], ["turn", "refresher"],
               ["turn", "refresher"], ["turn", "refresher"],
               ["set_interval", "item_2", 0.2], ["set_interval", "item_3", 0.2],
               ["set_interval", "item_4", 0.2], ["set_interval", "item_5", 0.2],
               ["clock", 1001.0],
               ["schedule", "item_1", null, false], ["schedule", "item_2", null, false],
               ["schedule", "item_3", 0.3, false], ["schedule", "item_4", null, false],
               ["schedule", "item_5", null, false], ["schedule", "item_8", 0.2, false],
               ["schedule", "item_8", 0.2, false],
               ["clock", 1001.1], ["cancel", "item_8"], ["schedule", "item_4", null, false],
               ["schedule", "item_5", null, true], ["set_value", "item_9", 9],
               ["set_interval", "item_2", null], ["set_interval", "item_3", null],
               ["set_interval", "item_4", null], ["set_interval", "item_5", null],
               ["set_interval", "item_7", null],
               ["turn", "invalidations"], ["turn", "invalidations"],
               ["clock", 1001.25], ["turn", "invalidations"], ["turn", "invalidations"],
               ["turn", "invalidations"], ["turn", "invalidations"], ["turn", "invalidations"],
               ["clock", 1001.45], ["turn", "invalidations"], ["turn", "invalidations"],
               ["turn", "invalidations"], ["turn", "invalidations"], ["turn", "invalidations"],
               ["turn", "invalidations"], ["turn", "invalidations"]]"#,
        )
        .unwrap(),
        // test_validation
        json!([["item", "valid_item", {"gather": [42], "validation": "only_42"}],
               ["item", "invalid_item", {"gather": [69], "validation": "only_42"}],
               ["add", "valid_item", true], ["add", "invalid_item", true],
               ["turn", "refresher"], ["turn", "refresher"], ["turn", "invalidations"]]),
        // test_gather_error
        json!([["item", "item", {"gather": [{"raise": "Test"}, {"raise": "Test"}],
                                 "on_fail_interval": 0.1}],
               ["add", "item", true], ["turn", "refresher"], ["turn", "invalidations"],
               ["clock", 1000.15], ["turn", "invalidations"], ["turn", "refresher"],
               ["cancel", "item"], ["set_value", "item", 42]]),
        // test_timeouts (the stuck gatherer: no refresher turn)
        json!([["item", "item_1", {"gather": [null], "timeout": 0.2}],
               ["item", "item_2", {"gather": [69, 69], "timeout": 0.2,
                                   "on_fail_interval": 1000, "validation": "only_42"}],
               ["item", "item_4", {"gather": [null], "timeout": 0.2}],
               ["item", "item_6", {"gather": [42], "timeout": 0.2}],
               ["add", "item_1", true], ["turn", "timeouts"], ["clock", 1000.25],
               ["turn", "timeouts"], ["turn", "timeouts"],
               ["add", "item_2", true], ["turn", "refresher"], ["clock", 1000.5],
               ["turn", "timeouts"], ["set_value", "item_2", 42], ["cancel", "item_2"],
               ["add", "item_4", true], ["clock", 1000.6], ["set_value", "item_4", 1],
               ["clock", 1001.0], ["turn", "timeouts"],
               ["add", "item_6", true], ["turn", "refresher"], ["clock", 1002.0],
               ["turn", "timeouts"], ["turn", "timeouts"], ["time_out", "item_6"]]),
        // test_empty_group
        json!([["group", "g", []]]),
        // test_group_invalidation
        json!([["item", "item_1", {"gather": [null, null]}],
               ["item", "item_2", {"gather": [null, null]}], ["group", "g", ["item_1", "item_2"]],
               ["add", "item_1", true], ["add", "item_2", true], ["turn", "refresher"],
               ["turn", "refresher"], ["invalidate_group", "g"]]),
        // test_garb
        json!([["item", "item", {"gather": [null]}], ["schedule", "item", null, false],
               ["invalidate", "item"], ["set_value", "item", 1], ["disable", "item"]]),
        // test_valid_group_init
        json!([["item", "item_1", {"gather": [null]}], ["item", "item_2", {"gather": [null]}],
               ["add", "item_1", true], ["add", "item_2", true], ["turn", "refresher"],
               ["turn", "refresher"], ["group", "g", ["item_1", "item_2"]],
               ["invalidate", "item_1"], ["turn", "refresher"]]),
        // test_valid_item_doesnt_gather
        json!([["item", "item_1", {"gather": [null]}], ["item", "item_2", {"gather": [69]}],
               ["add", "item_1", true], ["add", "item_2", true], ["set_value", "item_2", 42],
               ["turn", "refresher"], ["turn", "refresher"], ["turn", "refresher"]]),
        // test_subclasses_work
        json!([
            ["item", "my_item", {}],
            ["add", "my_item", true],
            ["turn", "refresher"]
        ]),
        // test_disabling
        json!([["item", "item", {"gather": [null, null], "interval": 0.2}],
               ["add", "item", false], ["turn", "refresher"], ["disable", "item"],
               ["disable", "item"], ["invalidate", "item"], ["turn", "refresher"],
               ["schedule", "item", 0.1, false], ["turn", "invalidations"], ["enable", "item"],
               ["enable", "item"], ["turn", "refresher"], ["disable", "item"],
               ["turn", "invalidations"]]),
        // test_group_updating
        json!([["item", "Item", {"gather": [null, null, null, null, null, null]}],
               ["add", "Item", false], ["group", "g", ["Item"]],
               ["on", "Item", "became_valid_signal", ["invalidate_group", "g"]],
               ["invalidate", "Item"], ["turn", "refresher"], ["turn", "refresher"],
               ["invalidate", "Item"], ["turn", "refresher"], ["turn", "refresher"]]),
        // side effects, values that do not change, None, the validator raising
        json!([["item", "a", {"gather": [{"side": true}, {"set": 7}, 7, null, 13],
                              "validation": "raise_13", "on_fail_interval": null}],
               ["add", "a", true], ["turn", "refresher"], ["invalidate", "a"],
               ["turn", "refresher"], ["invalidate", "a"], ["turn", "refresher"],
               ["invalidate", "a"], ["turn", "refresher"], ["set_value", "a", null],
               ["invalidate", "a"], ["turn", "refresher"], ["set_value", "a", 13],
               ["turn", "refresher"]]),
        // a gatherer that ran out (StopIteration), a value None first
        json!([["item", "a", {"gather": [null]}], ["add", "a", true], ["turn", "refresher"],
               ["invalidate", "a"], ["turn", "refresher"], ["turn", "invalidations"],
               ["clock", 1006.0], ["turn", "invalidations"], ["turn", "refresher"]]),
        // equal times in the timer queues: the names decide, the same item twice
        json!([["item", "c", {"gather": [1], "interval": 0.5, "timeout": 0.5}],
               ["item", "a", {"gather": [1], "interval": 0.5, "timeout": 0.5}],
               ["item", "b", {"gather": [1], "interval": 0.5, "timeout": 0.5}],
               ["item", "d", {"interval": 0.5, "timeout": 0.5}],
               ["add", "c", true], ["add", "a", true], ["add", "b", true], ["add", "d", true],
               ["schedule", "d", null, false], ["schedule", "d", null, true],
               ["schedule", "c", null, false], ["schedule", "b", 0.25, false],
               ["schedule", "a", 0.5, false],
               ["clock", 1000.5], ["turn", "invalidations"], ["turn", "invalidations"],
               ["turn", "invalidations"], ["turn", "invalidations"], ["turn", "invalidations"],
               ["turn", "invalidations"], ["turn", "timeouts"], ["turn", "timeouts"],
               ["turn", "timeouts"], ["turn", "timeouts"], ["turn", "timeouts"],
               ["turn", "refresher"], ["turn", "refresher"], ["turn", "refresher"],
               ["turn", "refresher"], ["turn", "refresher"]]),
        // an item in two groups
        json!([["sorted"], ["item", "a", {"gather": [1, 2]}], ["item", "b", {"gather": [1, 2]}],
               ["group", "g1", ["a", "b"]], ["group", "g2", ["a"]], ["add", "a", true],
               ["add", "b", true], ["turn", "refresher"], ["turn", "refresher"],
               ["invalidate", "a"], ["turn", "refresher"], ["invalidate_group", "g1"],
               ["turn", "refresher"], ["turn", "refresher"]]),
    ]
}

struct Case {
    updater: ItemUpdater,
    clock: Arc<Mutex<f64>>,
    items: indexmap::IndexMap<String, Arc<WatchedItem<V>>>,
    groups: indexmap::IndexMap<String, Arc<WatchedGroup>>,
    log: Log,
    keep: Vec<Arc<dyn Fn(&Arc<WatchedItem<V>>) + Send + Sync>>,
    keep_values: Vec<Arc<dyn Fn(&V) + Send + Sync>>,
    keep_groups: Vec<Arc<dyn Fn(&Arc<WatchedGroup>) + Send + Sync>>,
    /// the items, for the gatherers that set their own value
    shared: Arc<Mutex<HashMap<String, Arc<WatchedItem<V>>>>>,
}

fn num(x: Option<f64>) -> String {
    x.map_or("None".to_owned(), py_float_repr)
}

fn error(e: &PyErr) -> Value {
    let message = if e.class_name() == "KeyError" {
        ""
    } else {
        e.message()
    };
    json!([e.class_name(), message])
}

impl Case {
    fn new() -> Case {
        let clock = Arc::new(Mutex::new(1000.0));
        let c = clock.clone();
        Case {
            updater: ItemUpdater::with_clock(0.0, Arc::new(move || *c.lock().unwrap())),
            clock,
            items: Default::default(),
            groups: Default::default(),
            log: Default::default(),
            keep: vec![],
            keep_values: vec![],
            keep_groups: vec![],
            shared: Default::default(),
        }
    }

    fn signals(item: &WatchedItem<V>) -> [(&'static str, &Signal<Arc<WatchedItem<V>>>); 6] {
        [
            ("became_valid_signal", &item.became_valid_signal),
            ("became_invalid_signal", &item.became_invalid_signal),
            ("timed_out_signal", &item.timed_out_signal),
            ("error_refreshing_signal", &item.error_refreshing_signal),
            ("validation_error_signal", &item.validation_error_signal),
            ("val_err_timeout_signal", &item.val_err_timeout_signal),
        ]
    }

    fn make_item(&mut self, name: &str, args: &Value) {
        let log = self.log.clone();
        let mut item_args: ItemArgs<V> = ItemArgs::default();
        if let Some(script) = args.get("gather").and_then(Value::as_array) {
            let steps = Mutex::new(script.clone().into_iter());
            let (log, updater, shared, name) = (
                log.clone(),
                self.updater.clone(),
                self.shared.clone(),
                name.to_owned(),
            );
            item_args.gather_function = Some(Arc::new(move || {
                let Some(step) = steps.lock().unwrap().next() else {
                    return Err(GatherError::Raised(PyErr::Other(
                        "StopIteration",
                        String::new(),
                    )));
                };
                log.lock().unwrap().push(json!(["gather", name]));
                if let Some(step) = step.as_object() {
                    if let Some(text) = step.get("raise") {
                        return Err(GatherError::Raised(PyErr::Other(
                            "RuntimeError",
                            text.as_str().unwrap().to_owned(),
                        )));
                    }
                    if let Some(value) = step.get("set") {
                        let item = shared.lock().unwrap()[&name].clone();
                        updater
                            .set_value(&item, value.as_i64())
                            .map_err(GatherError::Raised)?;
                    }
                    return Err(GatherError::SideEffectOnly);
                }
                Ok(step.as_i64())
            }));
        }
        if let Some(kind) = args.get("validation").and_then(Value::as_str) {
            let kind = kind.to_owned();
            item_args.validation_function = Some(Arc::new(move |value: &V| {
                if kind == "only_42" {
                    return Ok(*value == Some(42));
                }
                if *value == Some(13) {
                    return Err(PyErr::Other("RuntimeError", "thirteen".to_owned()));
                }
                Ok(true)
            }));
        }
        {
            let (log, name) = (log.clone(), name.to_owned());
            item_args.write_function = Some(Arc::new(move |value: &V| {
                log.lock().unwrap().push(json!(["write", name, value]));
            }));
        }
        if let Some(v) = args.get("interval") {
            item_args.interval = v.as_f64();
        }
        if let Some(v) = args.get("timeout") {
            item_args.timeout = v.as_f64();
        }
        if let Some(v) = args.get("on_fail_interval") {
            item_args.on_fail_interval = v.as_f64();
        }
        let item = WatchedItem::new(name, item_args);
        for (signal, s) in Case::signals(&item) {
            let log = log.clone();
            let receiver: Arc<dyn Fn(&Arc<WatchedItem<V>>) + Send + Sync> =
                Arc::new(move |sender: &Arc<WatchedItem<V>>| {
                    log.lock().unwrap().push(json!([signal, sender.name]));
                });
            s.connect(&receiver, true);
            self.keep.push(receiver);
        }
        let log2 = log.clone();
        let receiver: Arc<dyn Fn(&V) + Send + Sync> = Arc::new(move |value: &V| {
            log2.lock()
                .unwrap()
                .push(json!(["value_changed_signal", value]));
        });
        item.value_changed_signal.connect(&receiver, true);
        self.keep_values.push(receiver);
        self.shared
            .lock()
            .unwrap()
            .insert(name.to_owned(), item.clone());
        self.items.insert(name.to_owned(), item);
    }

    fn make_group(&mut self, name: &str, members: &Value) -> Result<(), PyErr> {
        let members: Vec<Arc<dyn Tracked>> = members
            .as_array()
            .unwrap()
            .iter()
            .map(|m| self.items[m.as_str().unwrap()].clone() as Arc<dyn Tracked>)
            .collect();
        let group = WatchedGroup::new(members)?;
        for (signal, s) in [
            ("group became_valid_signal", &group.became_valid_signal),
            ("group became_invalid_signal", &group.became_invalid_signal),
        ] {
            let (log, name) = (self.log.clone(), name.to_owned());
            let receiver: Arc<dyn Fn(&Arc<WatchedGroup>) + Send + Sync> =
                Arc::new(move |_: &Arc<WatchedGroup>| {
                    log.lock().unwrap().push(json!([signal, name]));
                });
            s.connect(&receiver, true);
            self.keep_groups.push(receiver);
        }
        self.groups.insert(name.to_owned(), group);
        Ok(())
    }

    fn on(&mut self, item: &str, signal: &str, action: &Value) {
        let (updater, items, groups, action) = (
            self.updater.clone(),
            self.shared.clone(),
            self.groups.clone(),
            action.clone(),
        );
        let receiver: Arc<dyn Fn(&Arc<WatchedItem<V>>) + Send + Sync> =
            Arc::new(move |_: &Arc<WatchedItem<V>>| {
                let target = action[1].as_str().unwrap();
                // an exception in a receiver is the sender's: none of these raise here
                let _ = match action[0].as_str().unwrap() {
                    "invalidate_group" => updater.invalidate_group(&groups[target]),
                    "invalidate" => updater.invalidate(&items.lock().unwrap()[target].clone()),
                    "set_value" => updater
                        .set_value(&items.lock().unwrap()[target].clone(), action[2].as_i64()),
                    other => panic!("{other}"),
                };
            });
        let item = self.items[item].clone();
        let s = Case::signals(&item)
            .into_iter()
            .find(|(n, _)| *n == signal)
            .unwrap()
            .1;
        s.connect(&receiver, true);
        self.keep.push(receiver);
    }

    fn turn(&self, which: &str) -> Result<(), PyErr> {
        self.updater.set_events();
        match which {
            "refresher" => self.updater._refresher_once(),
            "invalidations" => self.updater._process_invalidations_once(),
            "timeouts" => self.updater._process_timeouts_once(),
            other => panic!("{other}"),
        }
    }

    fn state(&self) -> Value {
        let items: serde_json::Map<String, Value> = self
            .items
            .iter()
            .map(|(n, i)| {
                let s = i.state.lock().unwrap().clone();
                (
                    n.clone(),
                    json!([
                        s.valid,
                        s.scheduled,
                        num(s.interval),
                        s.disabled,
                        num(s.on_fail_interval),
                        num(s.timeout),
                        num(Some(s.invalidate_at)),
                        num(Some(s.times_out_at)),
                        i.value().flatten()
                    ]),
                )
            })
            .collect();
        let groups: serde_json::Map<String, Value> = self
            .groups
            .iter()
            .map(|(n, g)| {
                let sorted = |s: &Mutex<IndexSet<String>>| {
                    let mut v: Vec<String> = s.lock().unwrap().iter().cloned().collect();
                    v.sort();
                    v
                };
                (
                    n.clone(),
                    json!([
                        *g.valid.lock().unwrap(),
                        sorted(&g.valid_items),
                        sorted(&g.invalid_items)
                    ]),
                )
            })
            .collect();
        let mut tracked: Vec<String> = self.updater.items().into_iter().collect();
        tracked.sort();
        let (refresh, invalidate_timers, timeout_timers) = self.updater.queued();
        let timers = |t: Vec<(f64, String)>| {
            t.into_iter()
                .map(|(t, n)| json!([py_float_repr(t), n]))
                .collect::<Vec<_>>()
        };
        json!({
            "items": items,
            "groups": groups,
            "tracked": tracked,
            "refresh": refresh,
            "invalidate_timers": timers(invalidate_timers),
            "timeout_timers": timers(timeout_timers),
        })
    }

    fn run(mut self, ops: &Value) -> Vec<Value> {
        let mut steps = vec![];
        for op in ops.as_array().unwrap() {
            self.log.lock().unwrap().clear();
            let s = |i: usize| op[i].as_str().unwrap();
            let item = |i: usize| self.items[op[i].as_str().unwrap()].clone();
            let result: Result<(), PyErr> = match s(0) {
                "sorted" => Ok(()),
                "item" => {
                    self.make_item(s(1), &op[2]);
                    Ok(())
                }
                "group" => self.make_group(s(1), &op[2]),
                "on" => {
                    self.on(s(1), s(2), &op[3]);
                    Ok(())
                }
                "clock" => {
                    *self.clock.lock().unwrap() = op[1].as_f64().unwrap();
                    Ok(())
                }
                "add" => self.updater.add_item(&item(1), op[2].as_bool().unwrap()),
                "invalidate" => self.updater.invalidate(&item(1)),
                "invalidate_group" => self.updater.invalidate_group(&self.groups[s(1)]),
                "disable" => self.updater.disable(&item(1)),
                "enable" => self.updater.enable(&item(1)),
                "set_value" => self.updater.set_value(&item(1), op[2].as_i64()),
                "schedule" => self.updater.schedule_invalidation(
                    &item(1),
                    op[2].as_f64(),
                    op[3].as_bool().unwrap(),
                ),
                "cancel" => self.updater.cancel_scheduled_invalidation(&item(1)),
                "set_interval" => {
                    item(1).state.lock().unwrap().interval = op[2].as_f64();
                    Ok(())
                }
                "time_out" => {
                    ItemUpdater::_time_out(item(1).as_ref());
                    Ok(())
                }
                "turn" => self.turn(s(1)),
                other => panic!("{other}"),
            };
            let result = match result {
                Ok(()) => Value::Null,
                Err(e) => error(&e),
            };
            let log = self.log.lock().unwrap().clone();
            steps.push(json!([result, log, self.state()]));
        }
        steps
    }
}

fn sorted_logs(mut steps: Vec<Value>) -> Vec<Value> {
    for step in &mut steps {
        let log = step[1].as_array_mut().unwrap();
        log.sort_by_key(|v| v.to_string());
    }
    steps
}

/// PrusaLink's own `ItemUpdater`, `WatchedItem` and `WatchedGroup` and ours through the same
/// operations, the clock set by the case and the threads' loops one turn at a time: PrusaLink's
/// tests as operations, gatherers that raise, run out, only have side effects or set their own
/// value, validators that refuse or raise, values that stay the same or are `None`, disabled
/// items, a receiver that invalidates the group of the item that just became valid, timers with
/// equal times. After each: the same error, the same signals and writes in the same order, and
/// the same state of the items, the groups and the three queues (the timer heaps in their order).
#[test]
fn identical_to_prusalink() {
    let cases = cases();
    let Some(theirs) = python::run(
        include_str!("../driver_item_updater.py"),
        &json!({ "cases": cases }),
    ) else {
        return;
    };
    for (number, (ops, their)) in cases.iter().zip(theirs.as_array().unwrap()).enumerate() {
        let mut ours = Case::new().run(ops);
        let mut their = their.as_array().unwrap().clone();
        if ops[0][0] == "sorted" {
            ours = sorted_logs(ours);
            their = sorted_logs(their);
        }
        assert_eq!(their.len(), ours.len());
        for (step, (t, o)) in their.iter().zip(&ours).enumerate() {
            assert_eq!(t, o, "case {number} step {step}: {}", ops[step]);
        }
    }
}

/// CPython 3.13's own `queue.PriorityQueue` and ours through the same puts and gets of
/// `(time, item)` tuples, many with equal times and some with equal names: the same order out.
#[test]
fn priority_queue_identical_to_cpython() {
    let mut seed: u64 = 7;
    let mut next = |n: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % n
    };
    let mut ops = vec![];
    let mut size = 0;
    for _ in 0..3000 {
        if size > 0 && next(3) == 0 {
            ops.push(json!(["get"]));
            size -= 1;
        } else {
            let t = [0.5, 1.0, 1.0, 2.0, f64::INFINITY][next(5) as usize];
            let name = ["a", "b", "c", "item_1", "item_2"][next(5) as usize];
            ops.push(json!([
                "put",
                if t.is_infinite() {
                    json!("inf")
                } else {
                    json!(t)
                },
                name
            ]));
            size += 1;
        }
    }
    let driver = r#"
import queue
class Item:
    def __init__(self, name):
        self.name = name
    def __lt__(self, other):
        return self.name < other.name
    def __eq__(self, other):
        return self.name == other.name
    def __hash__(self):
        return hash(self.name)
q = queue.PriorityQueue()
out = []
for op in json.load(sys.stdin)["ops"]:
    if op[0] == "put":
        q.put((float(op[1]), Item(op[2])))
    else:
        t, item = q.get()
        out.append([repr(t), item.name, [[repr(x), i.name] for x, i in q.queue]])
print(json.dumps(out))
"#;
    let Some(theirs) = python::run(driver, &json!({ "ops": ops })) else {
        return;
    };
    struct Named(String);
    impl Tracked for Named {
        fn name(&self) -> &str {
            &self.0
        }
        fn lock(&self) -> &RLock {
            unreachable!()
        }
        fn state(&self) -> &Mutex<ItemState> {
            unreachable!()
        }
        fn in_groups(&self) -> Vec<Arc<WatchedGroup>> {
            unreachable!()
        }
        fn add_group(&self, _: &Arc<WatchedGroup>) {
            unreachable!()
        }
        fn send_became_invalid(&self) {
            unreachable!()
        }
        fn send_timed_out(&self) {
            unreachable!()
        }
        fn gather(&self, _: &ItemUpdater) -> Result<(), PyErr> {
            unreachable!()
        }
    }
    let q: Timers = Queue::new(0, PriorityQueue::new());
    let mut ours = vec![];
    for op in &ops {
        if op[0] == "put" {
            let t = op[1].as_f64().unwrap_or(f64::INFINITY);
            let item: Arc<dyn Tracked> = Arc::new(Named(op[2].as_str().unwrap().to_owned()));
            q.put((t, ItemKey(item)), true, None).unwrap();
        } else {
            let (t, ItemKey(item)) = q.get(true, None).unwrap();
            let rest: Vec<Value> = q
                .lock()
                .queue
                .queue
                .iter()
                .map(|(x, ItemKey(i))| json!([py_float_repr(*x), i.name()]))
                .collect();
            ours.push(json!([py_float_repr(t), item.name(), rest]));
        }
    }
    assert_eq!(theirs, json!(ours));
}

mod prusalink_tests {
    //! PrusaLink's `tests/test_item_updater.py` (`reference/Prusa-Link`, `171148ce`), test by test.
    //! The mocks of its `tests/util.py`: a `Mock` is a call log, a `WaitingMock` waits for its
    //! event and returns Python's `None`, an `EventSetMock` sets its event when called.

    use super::super::{
        GatherError, GatherFn, ItemArgs, ItemKey, ItemUpdater, ValidationFn, WatchedGroup,
        WatchedItem, WriteFn,
    };
    use crate::ports::prusalink::blinker::Receiver;
    use crate::ports::python::py_queue::{PriorityQueue, Queue};
    use crate::ports::python::py_threading::{Event, time_sleep};
    use maker_connect_gcode_metadata::py::PyErr;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// The threshold of the Python tests (`test_item_updater.py`, `:26`).
    const THRESHOLD: f64 = 0.05;

    /// The value of an item: Python's `None` or an integer.
    type Value = Option<i64>;
    /// An item of the tests.
    type Item = WatchedItem<Value>;
    /// An item, as the signals send it.
    type ItemArc = Arc<Item>;
    /// A group, as its signals send it.
    type GroupArc = Arc<WatchedGroup>;

    /// `time()`: when, in seconds, as the updater's own clock has it.
    fn now() -> f64 {
        crate::ports::prusalink::util::time()
    }

    /// What a mock records: comparable, and printable for an assertion's message.
    trait Recorded: Clone {
        /// Is this the same call as the other one's?
        fn same(&self, other: &Self) -> bool;
        /// How to print it.
        fn show(&self) -> String;
    }

    impl Recorded for () {
        fn same(&self, _other: &Self) -> bool {
            true
        }
        fn show(&self) -> String {
            "()".to_owned()
        }
    }

    impl Recorded for Value {
        fn same(&self, other: &Self) -> bool {
            self == other
        }
        fn show(&self) -> String {
            format!("{:?}", self)
        }
    }

    impl Recorded for ItemArc {
        fn same(&self, other: &Self) -> bool {
            self.name == other.name
        }
        fn show(&self) -> String {
            self.name.clone()
        }
    }

    impl Recorded for GroupArc {
        fn same(&self, other: &Self) -> bool {
            Arc::ptr_eq(self, other)
        }
        fn show(&self) -> String {
            "the group".to_owned()
        }
    }

    /// `tests/util.py`'s `Mock`: the calls it received, in order.
    struct Mock<A> {
        calls: Arc<Mutex<Vec<A>>>,
    }

    impl<A> Clone for Mock<A> {
        fn clone(&self) -> Self {
            Mock {
                calls: self.calls.clone(),
            }
        }
    }

    impl<A> Mock<A> {
        fn new() -> Self {
            Mock {
                calls: Arc::new(Mutex::new(vec![])),
            }
        }
        /// The call, with its arguments.
        fn call(&self, args: A) {
            self.calls.lock().unwrap().push(args);
        }
        /// How many times it was called.
        fn count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl<A: Recorded> Mock<A> {
        /// `assert_not_called()`
        fn assert_not_called(&self) {
            assert_eq!(self.count(), 0, "the mock was called");
        }
        /// `assert_called_once()`
        fn assert_called_once(&self) {
            assert_eq!(self.count(), 1, "the mock was called once");
        }
        /// `assert_called_once_with(...)`
        fn assert_called_once_with(&self, args: A) {
            let calls = self.calls.lock().unwrap();
            assert_eq!(calls.len(), 1, "the mock was called once");
            assert!(
                calls[0].same(&args),
                "the mock was called with {}, not {}",
                calls[0].show(),
                args.show()
            );
        }
    }

    impl Mock<()> {
        /// The mock as an item's `gather_function`, returning the value.
        fn gather(&self, value: Value) -> GatherFn<Value> {
            let calls = self.clone();
            Arc::new(move || {
                calls.call(());
                Ok(value)
            })
        }
    }

    impl Mock<Value> {
        /// The mock as an item's `write_function`.
        fn write(&self) -> WriteFn<Value> {
            let calls = self.clone();
            Arc::new(move |value: &Value| {
                calls.call(value.clone());
            })
        }
    }

    /// `tests/util.py`'s `WaitingMock`: waits for its event when called.
    struct WaitingMock {
        calls: Mock<()>,
        event: Arc<Event>,
        value: Value,
    }

    impl WaitingMock {
        /// `WaitingMock()`: waits for the event, then returns Python's `None`.
        fn new() -> Self {
            Self::with_return(None)
        }
        /// `WaitingMock(return_value=...)`: waits for the event, then returns the value.
        fn with_return(value: Value) -> Self {
            WaitingMock {
                calls: Mock::new(),
                event: Arc::new(Event::new()),
                value,
            }
        }
        /// The mock as an item's `gather_function`.
        fn gather(&self) -> GatherFn<Value> {
            let calls = self.calls.clone();
            let event = self.event.clone();
            let value = self.value;
            // A `Mock` records the call before its side effect, the wait.
            Arc::new(move || {
                calls.call(());
                event.wait(None);
                Ok(value)
            })
        }
        /// `assert_called_once()`
        fn assert_called_once(&self) {
            self.calls.assert_called_once();
        }
    }

    /// `tests/util.py`'s `EventSetMock`: sets its event when called.
    struct EventSetMock<A> {
        calls: Mock<A>,
        event: Arc<Event>,
    }

    impl<A: Recorded + Send + Sync + 'static> EventSetMock<A> {
        fn new() -> Self {
            EventSetMock {
                calls: Mock::new(),
                event: Arc::new(Event::new()),
            }
        }
        /// The mock as a signal's receiver: it gets the sender.
        fn receiver(&self) -> Arc<Receiver<A>> {
            let calls = self.calls.clone();
            let event = self.event.clone();
            Arc::new(move |sender: &A| {
                calls.call(sender.clone());
                event.set();
            })
        }
        /// `assert_not_called()`
        fn assert_not_called(&self) {
            self.calls.assert_not_called();
        }
        /// `assert_called_once()`
        fn assert_called_once(&self) {
            self.calls.assert_called_once();
        }
        /// `assert_called_once_with(...)`
        fn assert_called_once_with(&self, args: A) {
            self.calls.assert_called_once_with(args);
        }
    }

    impl EventSetMock<()> {
        /// The mock as an item's `gather_function`, returning the value.
        fn gather(&self, value: Value) -> GatherFn<Value> {
            let calls = self.calls.clone();
            let event = self.event.clone();
            Arc::new(move || {
                calls.call(());
                event.set();
                Ok(value)
            })
        }
    }

    impl EventSetMock<Value> {
        /// The mock as an item's `write_function`.
        fn write(&self) -> WriteFn<Value> {
            let calls = self.calls.clone();
            let event = self.event.clone();
            Arc::new(move |value: &Value| {
                calls.call(value.clone());
                event.set();
            })
        }
    }

    /// The `updater_instance` fixture: an `ItemUpdater`, started; stopped when the test ends,
    /// passed or failed (the fixture's `info_updater.stop()` after its `yield`, `:33-34`).
    ///
    /// These tests measure wall-clock time with PrusaLink's margins (`THRESHOLD`, and
    /// `test_scheduled_invalidation`'s deadline of `refresh_offset + THRESHOLD`, `:266-270`, which
    /// its results at `offset_interval` only meet because `sleep(0.1)` runs late). So they take
    /// turns: one updater's three threads at a time, the numbers as the source's.
    fn updater_instance() -> UpdaterInstance {
        static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
        let turn = ONE_AT_A_TIME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let updater = ItemUpdater::new(0.2);
        updater.start().unwrap();
        UpdaterInstance(updater, turn)
    }

    struct UpdaterInstance(ItemUpdater, #[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

    impl std::ops::Deref for UpdaterInstance {
        type Target = ItemUpdater;
        fn deref(&self) -> &ItemUpdater {
            &self.0
        }
    }

    impl Drop for UpdaterInstance {
        fn drop(&mut self) {
            self.0.stop();
        }
    }

    /// The `validator` fixture: accepts only 42 as a valid value.
    fn validator() -> ValidationFn<Value> {
        Arc::new(|value: &Value| Ok(value == &Some(42)))
    }

    /// `test_basics`: Tests the basics, adding a watched item, gathering and writing of a value
    /// and became valid/invalid signalling
    /// (`reference/Prusa-Link/tests/test_item_updater.py`, `47-84`).
    #[test]
    fn test_basics() {
        let updater = updater_instance();
        let gather = WaitingMock::with_return(Some(42));
        let write = Mock::<Value>::new();
        // This empty spec makes it possible to pass this mock straight to a
        // blinker signal
        let invalidated = EventSetMock::<ItemArc>::new();
        let valid = EventSetMock::<ItemArc>::new();
        let basic_item = WatchedItem::new(
            "basic_item",
            ItemArgs::<Value> {
                gather_function: Some(gather.gather()),
                write_function: Some(write.write()),
                ..ItemArgs::default()
            },
        );
        let valid_receiver = valid.receiver();
        let invalidated_receiver = invalidated.receiver();
        basic_item
            .became_valid_signal
            .connect(&valid_receiver, true);
        basic_item
            .became_invalid_signal
            .connect(&invalidated_receiver, true);
        updater.add_item(&basic_item, true).unwrap();

        // Reminder that invalidation is only signalled when going from a
        // valid state, so not at the beginning
        invalidated.assert_not_called();
        valid.assert_not_called();
        write.assert_not_called();

        gather.event.set(); // unstuck the gather
        assert!(valid.event.wait(Some(1.0)), "Didn't signal becoming valid");

        gather.assert_called_once();
        write.assert_called_once_with(Some(42));
        valid.assert_called_once_with(basic_item.clone());

        updater.invalidate(&basic_item).unwrap();
        assert!(
            invalidated.event.wait(Some(1.0)),
            "Didn't signal becoming invalid"
        );

        invalidated.assert_called_once_with(basic_item.clone());
    }

    /// `test_group`: Tests that the WatchedGroup becomes valid only after all its
    /// children are valid.
    /// Tests that the WatchedGroup becomes invalid, if it was valid and one of
    /// its members becomes invalid
    /// Tests that the WatchedGroup does not signal invalidation unless it was
    /// valid before
    /// (`reference/Prusa-Link/tests/test_item_updater.py`, `87-145`).
    #[test]
    fn test_group() {
        let updater = updater_instance();
        let gather_1 = WaitingMock::with_return(Some(1));
        let gather_2 = WaitingMock::with_return(Some(2));
        let group_valid = EventSetMock::<GroupArc>::new();
        let group_invalidated = EventSetMock::<GroupArc>::new();
        let item_1_valid = EventSetMock::<ItemArc>::new();
        let item_2_valid = EventSetMock::<ItemArc>::new();
        let item_2_invalidated = EventSetMock::<ItemArc>::new();
        let item_1 = WatchedItem::new(
            "item_1",
            ItemArgs::<Value> {
                gather_function: Some(gather_1.gather()),
                write_function: Some(Mock::<Value>::new().write()),
                ..ItemArgs::default()
            },
        );
        let item_1_valid_receiver = item_1_valid.receiver();
        item_1
            .became_valid_signal
            .connect(&item_1_valid_receiver, true);
        let item_2 = WatchedItem::new(
            "item_2",
            ItemArgs::<Value> {
                gather_function: Some(gather_2.gather()),
                write_function: Some(Mock::<Value>::new().write()),
                ..ItemArgs::default()
            },
        );
        let item_2_valid_receiver = item_2_valid.receiver();
        item_2
            .became_valid_signal
            .connect(&item_2_valid_receiver, true);
        let item_2_invalidated_receiver = item_2_invalidated.receiver();
        item_2
            .became_invalid_signal
            .connect(&item_2_invalidated_receiver, true);
        let watched_group = WatchedGroup::new(vec![item_1.clone(), item_2.clone()]).unwrap();
        let group_valid_receiver = group_valid.receiver();
        let group_invalidated_receiver = group_invalidated.receiver();
        watched_group
            .became_valid_signal
            .connect(&group_valid_receiver, true);
        watched_group
            .became_invalid_signal
            .connect(&group_invalidated_receiver, true);

        updater.add_item(&item_1, true).unwrap();
        updater.add_item(&item_2, true).unwrap();

        group_valid.assert_not_called();

        gather_1.event.set();

        assert!(item_1_valid.event.wait(Some(THRESHOLD)));

        group_valid.assert_not_called();

        gather_2.event.set();

        assert!(item_2_valid.event.wait(Some(THRESHOLD)));

        group_valid.assert_called_once();
        group_invalidated.assert_not_called();

        // Differs: `invalidate` queues item_1 to gather again, and `gather_1`'s event is still
        // set, so the gathering thread makes item_1 and the group valid again at once. Under the
        // GIL this thread runs on to `invalidate(item_2)` first (nothing between blocks); threads
        // here run at once, and a group valid again would become invalid a second time (`:145`).
        // So the second gather waits until the end.
        gather_1.event.clear();
        updater.invalidate(&item_1).unwrap();

        assert!(group_invalidated.event.wait(Some(THRESHOLD)));

        group_invalidated.assert_called_once();

        updater.invalidate(&item_2).unwrap();

        assert!(item_2_invalidated.event.wait(Some(THRESHOLD)));

        // Still only called once, not every time a member invalidates
        group_invalidated.assert_called_once();
        gather_1.event.set();
    }

    /// `test_scheduled_invalidation`: Tests that scheduling an invalidation works properly
    ///
    /// 1. Scheduling without an interval has to throw an error
    /// 2. Scheduling without an interval when the item has a default uses the it
    /// 3. Scheduling with an interval overwrites the default
    /// 4. Re-scheduling the interval does nothing
    /// 5. Force re-scheduling resets the interval
    /// 6. Setting the value resets the scheduled invalidation
    /// 7. Auto invalidation works
    /// 8. Cancelling a scheduled invalidation works
    /// 9. Setting a value to an item scheduled for invalidation without default
    ///    interval does not cancel the scheduling (should it work this way?)
    /// (`reference/Prusa-Link/tests/test_item_updater.py`, `148-282`).
    #[test]
    fn test_scheduled_invalidation() {
        let updater = updater_instance();
        let base_interval = 0.2; // base invalidation interval
        // for tests that refresh it, when to do so. should be < base_interval
        let refresh_offset = 0.1;
        let offset_interval = base_interval + refresh_offset;

        let time_of_start = Arc::new(Mutex::new(0.0)); // set up later to time()
        let results: Arc<Mutex<HashMap<i64, f64>>> = Arc::new(Mutex::new(HashMap::new()));

        // An item that records when it becomes invalid.
        let watched = |name: &str, interval: Option<f64>, key: i64| {
            let item = WatchedItem::new(
                name,
                ItemArgs::<Value> {
                    gather_function: Some(Mock::<()>::new().gather(None)),
                    interval,
                    ..ItemArgs::default()
                },
            );
            let results = results.clone();
            let time_of_start = time_of_start.clone();
            let receiver: Arc<Receiver<ItemArc>> = Arc::new(move |_: &ItemArc| {
                let elapsed = now() - *time_of_start.lock().unwrap();
                results.lock().unwrap().insert(key, elapsed);
            });
            item.became_invalid_signal.connect(&receiver, false);
            item
        };

        let item_1 = WatchedItem::new(
            "item_1",
            ItemArgs::<Value> {
                gather_function: Some(Mock::<()>::new().gather(None)),
                write_function: Some(Arc::new(|_: &Value| {})),
                ..ItemArgs::default()
            },
        );
        let item_2 = watched("item_2", None, 2);
        let item_3 = watched("item_3", None, 3);
        let item_4 = watched("item_4", None, 4);
        let item_5 = watched("item_5", None, 5);
        let item_6 = watched("item_6", None, 6);
        let item_7 = watched("item_7", Some(base_interval), 7);
        let item_8 = watched("item_8", Some(base_interval), 8);
        let item_9 = watched("item_9", Some(base_interval), 9);

        let group_valid = EventSetMock::<GroupArc>::new();
        let watched_group = WatchedGroup::new(vec![
            item_1.clone(),
            item_2.clone(),
            item_3.clone(),
            item_4.clone(),
            item_5.clone(),
            item_6.clone(),
            item_7.clone(),
            item_8.clone(),
            item_9.clone(),
        ])
        .unwrap();
        let group_valid_receiver = group_valid.receiver();
        watched_group
            .became_valid_signal
            .connect(&group_valid_receiver, true);

        updater.add_item(&item_1, true).unwrap();
        updater.add_item(&item_2, true).unwrap();
        updater.add_item(&item_3, true).unwrap();
        updater.add_item(&item_4, true).unwrap();
        updater.add_item(&item_5, true).unwrap();
        updater.add_item(&item_6, true).unwrap();
        updater.add_item(&item_7, true).unwrap();
        updater.add_item(&item_8, true).unwrap();
        updater.add_item(&item_9, true).unwrap();

        assert!(group_valid.event.wait(Some(THRESHOLD)));

        // set intervals after the items become valid for them to not auto schedule
        for item in [&item_2, &item_3, &item_4, &item_5, &item_6] {
            item.state.lock().unwrap().interval = Some(base_interval);
        }

        *time_of_start.lock().unwrap() = now();

        let failed = updater.schedule_invalidation(&item_1, None, false).is_err();
        assert!(
            failed,
            "Scheduling invalidation without an interval has to error"
        );

        updater.schedule_invalidation(&item_2, None, false).unwrap();
        updater
            .schedule_invalidation(&item_3, Some(base_interval + refresh_offset), false)
            .unwrap();

        updater.schedule_invalidation(&item_4, None, false).unwrap();
        updater.schedule_invalidation(&item_5, None, false).unwrap();
        updater.schedule_invalidation(&item_6, None, false).unwrap();
        updater
            .schedule_invalidation(&item_8, Some(base_interval), false)
            .unwrap();
        updater
            .schedule_invalidation(&item_8, Some(base_interval), false)
            .unwrap();

        time_sleep(refresh_offset);
        updater.cancel_scheduled_invalidation(&item_8).unwrap();
        updater.schedule_invalidation(&item_4, None, false).unwrap();
        updater.schedule_invalidation(&item_5, None, true).unwrap();
        updater.set_value(&item_6, Some(6)).unwrap();
        updater.set_value(&item_9, Some(9)).unwrap();

        // Reset the intervals to none, so the items won't auto re-schedule
        for item in [&item_2, &item_3, &item_4, &item_5, &item_6, &item_7] {
            item.state.lock().unwrap().interval = None;
        }

        // a "busy" wait in a test is fine
        let times_out_at = now() + refresh_offset + THRESHOLD;
        loop {
            let keys: Vec<i64> = results.lock().unwrap().keys().copied().collect();
            if [2, 3, 4, 5, 6, 7, 9].iter().all(|key| keys.contains(key)) {
                break;
            }
            if now() > times_out_at {
                panic!("Timed out waiting for invalidation results");
            }
            time_sleep(0.1);
        }

        {
            let results = results.lock().unwrap();
            assert!(base_interval <= results[&2] && results[&2] <= base_interval + THRESHOLD);
            assert!(offset_interval <= results[&3] && results[&3] <= offset_interval + THRESHOLD);
            assert!(base_interval <= results[&4] && results[&4] <= base_interval + THRESHOLD);
            assert!(offset_interval <= results[&5] && results[&5] <= offset_interval + THRESHOLD);
            assert!(offset_interval <= results[&6] && results[&6] <= offset_interval + THRESHOLD);
            assert!(offset_interval <= results[&9] && results[&9] <= offset_interval + THRESHOLD);

            // No precise syncing is done for this one, it's assumed it would have
            // already invalidated because we were waiting more than THRESHOLD after
            // it would if it was broken
            assert!(!results.contains_key(&8));
        }
    }

    /// `test_validation`: Tests that an item which gathers a valid value validates and
    /// that an item which gathers an invalid one errors out
    /// (`reference/Prusa-Link/tests/test_item_updater.py`, `285-314`).
    #[test]
    fn test_validation() {
        let updater = updater_instance();
        let valid_valid = EventSetMock::<ItemArc>::new();
        let valid_errored = EventSetMock::<ItemArc>::new();
        let valid_item = WatchedItem::new(
            "valid_item",
            ItemArgs::<Value> {
                gather_function: Some(Mock::<()>::new().gather(Some(42))),
                validation_function: Some(validator()),
                ..ItemArgs::default()
            },
        );
        let valid_valid_receiver = valid_valid.receiver();
        let valid_errored_receiver = valid_errored.receiver();
        valid_item
            .became_valid_signal
            .connect(&valid_valid_receiver, true);
        valid_item
            .validation_error_signal
            .connect(&valid_errored_receiver, true);
        updater.add_item(&valid_item, true).unwrap();

        let invalid_valid = EventSetMock::<ItemArc>::new();
        let invalid_errored = EventSetMock::<ItemArc>::new();
        let invalid_item = WatchedItem::new(
            "invalid_item",
            ItemArgs::<Value> {
                gather_function: Some(Mock::<()>::new().gather(Some(69))),
                validation_function: Some(validator()),
                ..ItemArgs::default()
            },
        );
        let invalid_valid_receiver = invalid_valid.receiver();
        let invalid_errored_receiver = invalid_errored.receiver();
        invalid_item
            .became_valid_signal
            .connect(&invalid_valid_receiver, true);
        invalid_item
            .validation_error_signal
            .connect(&invalid_errored_receiver, true);
        updater.add_item(&invalid_item, true).unwrap();

        assert!(valid_valid.event.wait(Some(THRESHOLD)));
        assert!(invalid_errored.event.wait(Some(THRESHOLD)));

        valid_valid.assert_called_once_with(valid_item.clone());
        valid_errored.assert_not_called();
        invalid_valid.assert_not_called();
        invalid_errored.assert_called_once();
    }

    /// `test_gather_error`: Test gather exception handling
    /// Test addressing items by their names
    /// (`reference/Prusa-Link/tests/test_item_updater.py`, `317-345`).
    #[test]
    fn test_gather_error() {
        let updater = updater_instance();
        let fail_interval = 0.1;
        let threshold = 0.05;

        let item_errored = EventSetMock::<ItemArc>::new();
        let write_mock = EventSetMock::<Value>::new();
        let item = WatchedItem::new(
            "item",
            ItemArgs::<Value> {
                gather_function: Some(Arc::new(|| {
                    Err(GatherError::Raised(PyErr::Other(
                        "RuntimeError",
                        "Test".to_owned(),
                    )))
                })),
                write_function: Some(write_mock.write()),
                on_fail_interval: Some(fail_interval),
                ..ItemArgs::default()
            },
        );
        let item_errored_receiver = item_errored.receiver();
        item.error_refreshing_signal
            .connect(&item_errored_receiver, true);

        let time_of_start = now();
        updater.add_item(&item, true).unwrap();
        assert!(item_errored.event.wait(Some(threshold)));
        item_errored.event.clear();
        assert!(item_errored.event.wait(Some(fail_interval + threshold)));
        let elapsed = now() - time_of_start;
        assert!(fail_interval < elapsed && elapsed < fail_interval + threshold);

        write_mock.assert_not_called();
        updater.cancel_scheduled_invalidation(&item).unwrap();
        updater.set_value(&item, Some(42)).unwrap();
        assert!(write_mock.event.wait(Some(threshold)));
    }

    /// `test_timeouts`: 1. Test that a stuck getter times out
    /// 2. Test that a failed getter which doesn't get re-scheduled times out
    /// 3. Test that a getter that keeps failing times out
    /// 4. Test that a getter which got stuck does not time out
    ///    if the value is provided from the outside
    /// 5. Test that an item which refreshes and then gets stuck times out after
    ///    the full time out amount
    /// 6. Test that an item which refreshes successfully doesn't time out
    /// (`reference/Prusa-Link/tests/test_item_updater.py`, `348-453`).
    #[test]
    fn test_timeouts() {
        let updater = updater_instance();
        let base_interval = 0.2; // base timeout interval
        let wait_interval = 0.1;
        let offset_interval = base_interval + wait_interval;

        let stuck_gatherer = WaitingMock::new();

        let item_1_timeout_mock = EventSetMock::<ItemArc>::new();
        let item_1 = WatchedItem::new(
            "item_1",
            ItemArgs::<Value> {
                gather_function: Some(stuck_gatherer.gather()),
                timeout: Some(base_interval),
                ..ItemArgs::default()
            },
        );
        let item_1_timeout_receiver = item_1_timeout_mock.receiver();
        item_1
            .timed_out_signal
            .connect(&item_1_timeout_receiver, true);

        let item_2_timeout_mock = EventSetMock::<ItemArc>::new();
        let item_2 = WatchedItem::new(
            "item_2",
            ItemArgs::<Value> {
                gather_function: Some(Mock::<()>::new().gather(Some(69))),
                timeout: Some(base_interval),
                on_fail_interval: Some(1000.0),
                validation_function: Some(validator()),
                ..ItemArgs::default()
            },
        );
        let item_2_timeout_receiver = item_2_timeout_mock.receiver();
        item_2
            .timed_out_signal
            .connect(&item_2_timeout_receiver, true);

        let item_3_timeout_mock = EventSetMock::<ItemArc>::new();
        let item_3 = WatchedItem::new(
            "item_3",
            ItemArgs::<Value> {
                gather_function: Some(Mock::<()>::new().gather(Some(69))),
                timeout: Some(base_interval),
                on_fail_interval: Some(1000.0),
                validation_function: Some(validator()),
                ..ItemArgs::default()
            },
        );
        let item_3_timeout_receiver = item_3_timeout_mock.receiver();
        item_3
            .timed_out_signal
            .connect(&item_3_timeout_receiver, true);

        let item_4_timeout_mock = EventSetMock::<ItemArc>::new();
        let item_4 = WatchedItem::new(
            "item_4",
            ItemArgs::<Value> {
                gather_function: Some(stuck_gatherer.gather()),
                timeout: Some(base_interval),
                ..ItemArgs::default()
            },
        );
        let item_4_timeout_receiver = item_4_timeout_mock.receiver();
        item_4
            .timed_out_signal
            .connect(&item_4_timeout_receiver, true);

        let item_5_timeout_mock = EventSetMock::<ItemArc>::new();
        let item_5_valid_mock = EventSetMock::<ItemArc>::new();
        let item_5 = WatchedItem::new(
            "item_5",
            ItemArgs::<Value> {
                gather_function: Some(stuck_gatherer.gather()),
                timeout: Some(base_interval),
                ..ItemArgs::default()
            },
        );
        let item_5_timeout_receiver = item_5_timeout_mock.receiver();
        let item_5_valid_receiver = item_5_valid_mock.receiver();
        item_5
            .timed_out_signal
            .connect(&item_5_timeout_receiver, true);
        item_5
            .became_valid_signal
            .connect(&item_5_valid_receiver, true);

        let item_6_timeout_mock = EventSetMock::<ItemArc>::new();
        let item_6 = WatchedItem::new(
            "item_6",
            ItemArgs::<Value> {
                gather_function: Some(Mock::<()>::new().gather(Some(42))),
                timeout: Some(base_interval),
                ..ItemArgs::default()
            },
        );
        let item_6_timeout_receiver = item_6_timeout_mock.receiver();
        item_6
            .timed_out_signal
            .connect(&item_6_timeout_receiver, true);

        let time_of_start = now();
        updater.add_item(&item_1, true).unwrap();
        assert!(
            item_1_timeout_mock
                .event
                .wait(Some(base_interval + THRESHOLD))
        );
        let elapsed = now() - time_of_start;
        assert!(base_interval < elapsed && elapsed < base_interval + THRESHOLD);
        stuck_gatherer.event.set();
        stuck_gatherer.event.clear();

        let time_of_start = now();
        updater.add_item(&item_2, true).unwrap();
        assert!(
            item_2_timeout_mock
                .event
                .wait(Some(base_interval + THRESHOLD))
        );
        let elapsed = now() - time_of_start;
        assert!(base_interval < elapsed && elapsed < base_interval + THRESHOLD);
        updater.set_value(&item_2, Some(42)).unwrap();
        // make sure this does not get invalidated
        updater.cancel_scheduled_invalidation(&item_2).unwrap();

        let time_of_start = now();
        updater.add_item(&item_3, true).unwrap();
        assert!(
            item_3_timeout_mock
                .event
                .wait(Some(base_interval + THRESHOLD))
        );
        let elapsed = now() - time_of_start;
        assert!(base_interval < elapsed && elapsed < base_interval + THRESHOLD);
        updater.set_value(&item_3, Some(42)).unwrap();
        // make sure this does not get invalidated
        updater.cancel_scheduled_invalidation(&item_3).unwrap();

        updater.add_item(&item_4, true).unwrap();
        time_sleep(wait_interval);
        updater.set_value(&item_4, Some(1)).unwrap();
        assert!(
            !item_4_timeout_mock
                .event
                .wait(Some(base_interval + THRESHOLD))
        );
        stuck_gatherer.event.set();
        stuck_gatherer.event.clear();

        let time_of_start = now();
        updater.add_item(&item_5, true).unwrap();
        time_sleep(wait_interval);
        stuck_gatherer.event.set();
        stuck_gatherer.event.clear();
        assert!(item_5_valid_mock.event.wait(Some(THRESHOLD)));
        updater.invalidate(&item_5).unwrap();
        assert!(
            item_5_timeout_mock
                .event
                .wait(Some(base_interval + THRESHOLD))
        );
        // It is important that it's not less than the offset interval
        let offset_with_threshold = offset_interval + THRESHOLD;
        let elapsed = now() - time_of_start;
        assert!(offset_interval < elapsed && elapsed < offset_with_threshold);
        stuck_gatherer.event.set();
        stuck_gatherer.event.clear();

        updater.add_item(&item_6, true).unwrap();
        assert!(
            !item_6_timeout_mock
                .event
                .wait(Some(base_interval + THRESHOLD))
        );
    }

    /// `test_empty_group`: Test that an empty group raises an error on creation
    /// (`reference/Prusa-Link/tests/test_item_updater.py`, `456-459`).
    #[test]
    fn test_empty_group() {
        let Err(error) = WatchedGroup::new(vec![]) else {
            panic!("an empty group is an error");
        };
        assert!(error.is_value_error());
    }

    /// `test_group_invalidation`: Test that group invalidation invalidates every member
    /// (`reference/Prusa-Link/tests/test_item_updater.py`, `462-482`).
    #[test]
    fn test_group_invalidation() {
        let updater = updater_instance();
        let item_1_invalidated = EventSetMock::<ItemArc>::new();
        let item_1 = WatchedItem::new(
            "item_1",
            ItemArgs::<Value> {
                gather_function: Some(Mock::<()>::new().gather(None)),
                ..ItemArgs::default()
            },
        );
        let item_1_invalidated_receiver = item_1_invalidated.receiver();
        item_1
            .became_invalid_signal
            .connect(&item_1_invalidated_receiver, true);
        let item_2_invalidated = EventSetMock::<ItemArc>::new();
        let item_2 = WatchedItem::new(
            "item_2",
            ItemArgs::<Value> {
                gather_function: Some(Mock::<()>::new().gather(None)),
                ..ItemArgs::default()
            },
        );
        let item_2_invalidated_receiver = item_2_invalidated.receiver();
        item_2
            .became_invalid_signal
            .connect(&item_2_invalidated_receiver, true);
        let group = WatchedGroup::new(vec![item_1.clone(), item_2.clone()]).unwrap();

        let group_validated = EventSetMock::<GroupArc>::new();
        let group_validated_receiver = group_validated.receiver();
        group
            .became_valid_signal
            .connect(&group_validated_receiver, true);

        updater.add_item(&item_1, true).unwrap();
        updater.add_item(&item_2, true).unwrap();

        assert!(group_validated.event.wait(Some(THRESHOLD)));

        updater.invalidate_group(&group).unwrap();
        assert!(item_1_invalidated.event.wait(Some(THRESHOLD)));
        assert!(item_2_invalidated.event.wait(Some(THRESHOLD)));
    }

    /// `test_garb`: Tests that addressing a non existing item throws a ValueError error
    /// Tests that adding a garbage for tracking fails
    /// (`reference/Prusa-Link/tests/test_item_updater.py`, `485-492`).
    #[test]
    fn test_garb() {
        let updater = updater_instance();
        let item = WatchedItem::new(
            "item",
            ItemArgs::<Value> {
                gather_function: Some(Mock::<()>::new().gather(None)),
                ..ItemArgs::default()
            },
        );
        let Err(error) = updater.schedule_invalidation(&item, None, false) else {
            panic!("an untracked item is an error");
        };
        assert!(error.is_value_error());
    }

    /// `test_valid_group_init`: Test that adding valid items to a WatchedGroup works too
    /// (`reference/Prusa-Link/tests/test_item_updater.py`, `495-525`).
    #[test]
    fn test_valid_group_init() {
        let updater = updater_instance();
        let item_1_valid = EventSetMock::<ItemArc>::new();
        let item_1 = WatchedItem::new(
            "item_1",
            ItemArgs::<Value> {
                gather_function: Some(Mock::<()>::new().gather(None)),
                ..ItemArgs::default()
            },
        );
        let item_1_valid_receiver = item_1_valid.receiver();
        item_1
            .became_valid_signal
            .connect(&item_1_valid_receiver, true);

        let item_2_valid = EventSetMock::<ItemArc>::new();
        let item_2 = WatchedItem::new(
            "item_2",
            ItemArgs::<Value> {
                gather_function: Some(Mock::<()>::new().gather(None)),
                ..ItemArgs::default()
            },
        );
        let item_2_valid_receiver = item_2_valid.receiver();
        item_2
            .became_valid_signal
            .connect(&item_2_valid_receiver, true);

        updater.add_item(&item_1, true).unwrap();
        updater.add_item(&item_2, true).unwrap();

        assert!(item_1_valid.event.wait(Some(THRESHOLD)));
        assert!(item_2_valid.event.wait(Some(THRESHOLD)));

        let group_valid = EventSetMock::<GroupArc>::new();
        let group_invalid = EventSetMock::<GroupArc>::new();

        let group = WatchedGroup::new(vec![item_1.clone(), item_2.clone()]).unwrap();
        let group_valid_receiver = group_valid.receiver();
        let group_invalid_receiver = group_invalid.receiver();
        group
            .became_valid_signal
            .connect(&group_valid_receiver, true);
        // The original connects the second mock to the valid signal, too.
        group
            .became_valid_signal
            .connect(&group_invalid_receiver, true);

        updater.invalidate(&item_1).unwrap();

        assert!(group_invalid.event.wait(Some(THRESHOLD)));
        assert!(group_valid.event.wait(Some(THRESHOLD)));
    }

    /// `test_valid_item_doesnt_gather`: Test that an item which became valid while being
    /// scheduled for gather does not actually gather
    /// (`reference/Prusa-Link/tests/test_item_updater.py`, `528-547`).
    #[test]
    fn test_valid_item_doesnt_gather() {
        let updater = updater_instance();
        let item_1_gather = WaitingMock::new();
        let item_1 = WatchedItem::new(
            "item_1",
            ItemArgs::<Value> {
                gather_function: Some(item_1_gather.gather()),
                ..ItemArgs::default()
            },
        );

        let item_2_gather = EventSetMock::<()>::new();
        let item_2 = WatchedItem::new(
            "item_2",
            ItemArgs::<Value> {
                gather_function: Some(item_2_gather.gather(Some(69))),
                ..ItemArgs::default()
            },
        );

        updater.add_item(&item_1, true).unwrap();
        updater.add_item(&item_2, true).unwrap();

        updater.set_value(&item_2, Some(42)).unwrap();
        item_1_gather.event.set();

        // Check that even when the get got unstuck an already valid item
        // does not gather (Or should it?)
        assert!(!item_2_gather.event.wait(Some(THRESHOLD)));
    }

    /// `test_subclasses_work`: Tests a subclass validates when being added to the Updater
    /// (`reference/Prusa-Link/tests/test_item_updater.py`, `550-557`).
    #[test]
    fn test_subclasses_work() {
        let updater = updater_instance();
        // A `WatchedItem` subclass is a `WatchedItem` here, too: the types have no subclasses.
        let my_item = WatchedItem::new("my_item", ItemArgs::<Value>::default());
        updater.add_item(&my_item, true).unwrap();
        assert!(updater.items().contains("my_item"));
    }

    /// `test_disabling`: Can we disable and enable the item updating without affecting
    /// any interval logic?
    /// (`reference/Prusa-Link/tests/test_item_updater.py`, `560-582`).
    #[test]
    fn test_disabling() {
        let updater = updater_instance();
        let item_gather = EventSetMock::<()>::new();
        let item = WatchedItem::new(
            "item",
            ItemArgs::<Value> {
                gather_function: Some(item_gather.gather(None)),
                interval: Some(0.2),
                ..ItemArgs::default()
            },
        );

        let item_valid = EventSetMock::<ItemArc>::new();
        let item_valid_receiver = item_valid.receiver();
        item.became_valid_signal.connect(&item_valid_receiver, true);

        updater.add_item(&item, false).unwrap();
        assert!(!item_gather.event.wait(Some(THRESHOLD)));
        assert_eq!(item.state.lock().unwrap().interval, Some(0.2));
        updater.disable(&item).unwrap();
        updater.invalidate(&item).unwrap();
        assert!(
            !item_gather.event.wait(Some(THRESHOLD)),
            "Do not invalidate disabled items"
        );
        updater
            .schedule_invalidation(&item, Some(0.1), false)
            .unwrap();
        assert!(
            !item_gather.event.wait(Some(THRESHOLD)),
            "Do not invalidate disabled items"
        );
        updater.enable(&item).unwrap();
        assert!(item_gather.event.wait(Some(THRESHOLD)));
        // Differs: the mock's event is set when the gather is called, `invalidate_at` only
        // after it returns (`_set_value` -> `schedule_invalidation`, `:370`). Under the GIL
        // the gathering thread gets that far before this one runs again; threads here run at
        // once, so this waits for the item to become valid first.
        assert!(item_valid.event.wait(Some(THRESHOLD)));
        let interval = item.state.lock().unwrap().interval.unwrap();
        assert!(item.state.lock().unwrap().invalidate_at <= now() + interval);
        updater.disable(&item).unwrap();
        assert_eq!(item.state.lock().unwrap().invalidate_at, f64::INFINITY);
    }

    /// `test_group_updating`: Test a bug, where if a became_valid handler invalidated the
    /// same item that just became balid, sometimes, the group the item was in got
    /// notifiied after all has been done and got confused to the point of
    /// raising a KeyError
    /// (`reference/Prusa-Link/tests/test_item_updater.py`, `585-602`).
    #[test]
    fn test_group_updating() {
        let updater = updater_instance();
        let item_gather = EventSetMock::<()>::new();
        let item = WatchedItem::new(
            "Item",
            ItemArgs::<Value> {
                gather_function: Some(item_gather.gather(None)),
                ..ItemArgs::default()
            },
        );
        updater.add_item(&item, false).unwrap();
        let group = WatchedGroup::new(vec![item.clone()]).unwrap();
        let updater_of_the_handler = updater.clone();
        let group_of_the_handler = group.clone();
        let receiver: Arc<Receiver<ItemArc>> = Arc::new(move |_: &ItemArc| {
            let _ = updater_of_the_handler.invalidate_group(&group_of_the_handler);
        });
        item.became_valid_signal.connect(&receiver, false);
        for _ in 0..100 {
            updater.invalidate(&item).unwrap();
            item_gather.event.wait(Some(THRESHOLD));
            item_gather.event.clear();
        }
    }

    /// `test_priority_queue`: Tests that you can have two watched items with the same priority
    /// and the app does not throw an error
    /// (`reference/Prusa-Link/tests/test_item_updater.py`, `605-614`).
    #[test]
    fn test_priority_queue() {
        let item_1 = WatchedItem::new("item_1", ItemArgs::<Value>::default());
        let item_2 = WatchedItem::new("item_2", ItemArgs::<Value>::default());
        let queue = Queue::new(0, PriorityQueue::new());
        queue
            .put((1.0, ItemKey(item_1.clone())), true, None)
            .unwrap();
        queue
            .put((1.0, ItemKey(item_2.clone())), true, None)
            .unwrap();
        queue.get(true, None).unwrap();
        queue.get(true, None).unwrap();
    }
}
