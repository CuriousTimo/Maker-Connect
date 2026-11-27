use super::*;
use crate::ports::prusalink::python;
use serde_json::{Value, json};

/// The constants of PrusaLink's own tests (`tests/test_is_planner_fed.py:18-26`): a small queue,
/// the dynamic threshold on, even heaps.
fn small() -> Value {
    json!({"QUEUE_SIZE": 10, "USE_DYNAMIC_THRESHOLD": true, "DEFAULT_THRESHOLD": 0.5,
           "HEAP_RATIO": 0.5, "IGNORE_ABOVE": 10.0})
}

fn values(v: impl IntoIterator<Item = f64>) -> Value {
    json!(v.into_iter().collect::<Vec<_>>())
}

/// Each case a list of operations (see `driver_is_planner_fed.py`). The first ones are PrusaLink's
/// own tests (`tests/test_is_planner_fed.py`) as operations, in their order.
fn cases() -> Vec<Value> {
    let p = "data/threshold.data";
    let mut cases = vec![
        // test_defaults
        json!([["new", p, small()]]),
        // test_threshold_from_the_file
        json!([
            ["write", "threshold.data", b"0.25"],
            ["new", "threshold.data", small()],
            ["write", "threshold.data", b"garbage"],
            ["new", "threshold.data", small()]
        ]),
        // test_static_threshold
        json!([["new", "t", {"QUEUE_SIZE": 10, "USE_DYNAMIC_THRESHOLD": false,
                             "DEFAULT_THRESHOLD": 0.5, "HEAP_RATIO": 0.5, "IGNORE_ABOVE": 10.0}],
               ["process_many", values([1.0; 12])]]),
        // test_dynamic_threshold_empty, test_values_above_the_limit_are_ignored
        json!([["new", p, small()], ["process", 11.0]]),
        // test_fed_decision
        json!([
            ["new", p, small()],
            ["process", 0.1],
            ["process", 1.0],
            ["process", 0.1]
        ]),
        // test_first_values_fill_the_heaps
        json!([
            ["new", p, small()],
            ["process", 2.0],
            ["process", 1.0],
            ["process", 3.0]
        ]),
        // test_second_larger_value
        json!([["new", p, small()], ["process", 1.0], ["process", 2.0]]),
        // test_dynamic_threshold_after_filling_the_queue
        json!([
            ["new", p, small()],
            ["process_many", values((1..=10).map(|v| v as f64 / 10.0))]
        ]),
        // test_old_values_are_dropped
        json!([
            ["new", p, small()],
            [
                "process_many",
                values((1..40).map(|v| (v % 7 + 1) as f64 / 10.0))
            ]
        ]),
        // test_heaps_stay_ordered
        json!([
            ["new", p, small()],
            [
                "process_many",
                values([
                    5.0, 1.0, 9.0, 3.0, 7.0, 2.0, 8.0, 4.0, 6.0, 0.5, 9.5, 1.5, 2.5
                ])
            ]
        ]),
        // test_balance_detects_broken_heaps
        json!([
            ["new", p, small()],
            ["short_push", 5.0],
            ["long_push", 1.0],
            ["balance"]
        ]),
        // test_save_only_with_a_full_queue
        json!([
            ["new", p, small()],
            ["process", 1.0],
            ["save"],
            [
                "process_many",
                values((0..10).map(|v| (v + 1) as f64 / 10.0))
            ],
            ["save"],
            ["new", p, small()]
        ]),
        // the threshold file: whitespace, CRLF, underscores, inf, nan, not UTF-8, a folder, empty
        json!([
            ["write", "a", b" 0.25\n"],
            ["new", "a", null],
            ["write", "a", b"0.5\r\n"],
            ["new", "a", null],
            ["write", "a", b"1_0"],
            ["new", "a", null],
            ["write", "a", b"inf"],
            ["new", "a", null],
            ["write", "a", b"nan"],
            ["new", "a", null],
            ["write", "a", b"-1e-05"],
            ["new", "a", null],
            ["write", "a", [0x30, 0x2e, 0xff]],
            ["new", "a", null],
            ["write", "a", b""],
            ["new", "a", null],
            ["mkdir", "folder"],
            ["new", "folder", null],
            ["new", "raw:threshold.data", null],
            ["new", "x//y/./z", null]
        ]),
        // the static threshold ignores the file
        json!([["write", "a", b"0.25"],
               ["new", "a", {"USE_DYNAMIC_THRESHOLD": false}]]),
        // a queue of 0 and of 1, an uneven ratio, broken heaps to remove from
        json!([["new", p, {"QUEUE_SIZE": 0}], ["process", 0.1], ["save"]]),
        json!([["new", p, {"QUEUE_SIZE": 1}], ["process", 0.1], ["process", 0.2],
               ["process", 0.05], ["save"]]),
        json!([["new", p, {"QUEUE_SIZE": 2}], ["process", 0.1], ["process", 0.2],
               ["process", 0.3], ["process", 0.01]]),
        json!([["new", p, {"QUEUE_SIZE": 20}],
               ["process_many", values((0..60).map(|v| ((v * 37) % 23) as f64 / 50.0))],
               ["save"], ["new", p, {"QUEUE_SIZE": 20}]]),
        json!([
            ["new", p, small()],
            ["remove_last"],
            ["balance"],
            ["long_push", 0.3],
            ["process", 0.2]
        ]),
    ];

    // const.py's own constants: a queue of 10000, the 95th percentile, over 25000 confirmations
    let mut seed: u64 = 245;
    let mut next = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as f64 / (1u64 << 31) as f64
    };
    let times: Vec<f64> = (0..25_000)
        .map(|i| {
            let r = next();
            // mostly quick, sometimes slow (a full planner), sometimes over IGNORE_ABOVE
            if i % 97 == 0 {
                1.0 + r
            } else if r < 0.1 {
                0.13 + r * 3.0
            } else {
                (r * 1000.0).round() / 10000.0
            }
        })
        .collect();
    cases.push(json!([
        ["new", p, null],
        ["process_many", times],
        ["save"],
        ["new", p, null]
    ]));
    cases
}

fn text(v: impl ToString, base: &str) -> String {
    v.to_string().replace(base, "BASE")
}

fn shown(r: Result<f64, PyErr>, base: &str) -> Value {
    match r {
        Ok(v) => json!(py_float_repr(v)),
        Err(e) => error(&e, base),
    }
}

fn error(e: &PyErr, base: &str) -> Value {
    json!([e.class_name(), text(e.message(), base)])
}

fn heap(h: &MinHeap<TimeValue>) -> Value {
    json!(
        h.heap
            .iter()
            .map(|i| {
                let i = i.lock().unwrap();
                json!([
                    py_float_repr(i.item.value),
                    i.item.heap_value.map(py_float_repr),
                    i.item.heap_index,
                    i.heap_name.map(HeapName::value)
                ])
            })
            .collect::<Vec<_>>()
    )
}

fn state(planner: &Option<IsPlannerFed>, base: &str) -> Value {
    let Some(planner) = planner else {
        return Value::Null;
    };
    let contents = std::path::Path::new(&planner.threshold_path)
        .is_file()
        .then(|| std::fs::read(&planner.threshold_path).unwrap());
    json!({
        "is_fed": planner.call(),
        "default_threshold": py_float_repr(planner.default_threshold),
        "item_count": planner.item_count(),
        "threshold": shown(planner.threshold(), base),
        "dynamic": shown(planner.get_dynamic_threshold(), base),
        "short": heap(&planner.short_times),
        "long": heap(&planner.long_times),
        "queue": planner.times_queue.iter().map(|i| {
            let i = i.lock().unwrap();
            json!([py_float_repr(i.item.value), i.heap_name.map(HeapName::value)])
        }).collect::<Vec<_>>(),
        "file": contents,
        "path": text(&planner.threshold_path, base),
    })
}

fn constants(patch: &Value) -> Constants {
    let mut c = Constants::default();
    let Some(patch) = patch.as_object() else {
        return c;
    };
    for (name, value) in patch {
        match name.as_str() {
            "QUEUE_SIZE" => c.queue_size = value.as_u64().unwrap() as usize,
            "HEAP_RATIO" => c.heap_ratio = value.as_f64().unwrap(),
            "IGNORE_ABOVE" => c.ignore_above = value.as_f64().unwrap(),
            "DEFAULT_THRESHOLD" => c.default_threshold = value.as_f64().unwrap(),
            "USE_DYNAMIC_THRESHOLD" => c.use_dynamic_threshold = value.as_bool().unwrap(),
            other => panic!("{other}"),
        }
    }
    c
}

/// Ours through the same operations as the driver, the folders under `base`.
fn ours(cases: &[Value], base: &str) -> Value {
    let mut out = vec![];
    for (number, ops) in cases.iter().enumerate() {
        let folder = format!("{base}/{number}");
        std::fs::create_dir_all(&folder).unwrap();
        let mut planner: Option<IsPlannerFed> = None;
        let mut steps = vec![];
        for op in ops.as_array().unwrap() {
            let arg = |i: usize| op[i].as_str().unwrap();
            let num = || op[1].as_f64().unwrap();
            let p = planner.as_mut();
            let result: Result<Value, PyErr> = match op[0].as_str().unwrap() {
                "new" => {
                    let path = match arg(1).strip_prefix("raw:") {
                        Some(raw) => raw.to_owned(),
                        None => format!("{folder}/{}", arg(1)),
                    };
                    planner = None;
                    IsPlannerFed::with_constants(&path, constants(&op[2]))
                        .map(|made| planner = Some(made))
                        .map(|_| Value::Null)
                }
                "write" => {
                    let full = format!("{folder}/{}", arg(1));
                    let bytes: Vec<u8> = op[2]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|b| b.as_u64().unwrap() as u8)
                        .collect();
                    std::fs::write(full, bytes).unwrap();
                    Ok(Value::Null)
                }
                "mkdir" => {
                    std::fs::create_dir_all(format!("{folder}/{}", arg(1))).unwrap();
                    Ok(Value::Null)
                }
                "process" => p.unwrap().process_value(num()).map(|_| Value::Null),
                "process_many" => {
                    let p = p.unwrap();
                    let mut seen = vec![];
                    let mut failed = None;
                    for value in op[1].as_array().unwrap() {
                        if let Err(e) = p.process_value(value.as_f64().unwrap()) {
                            failed = Some(e);
                            break;
                        }
                        seen.push(json!([p.call(), shown(p.threshold(), base)]));
                    }
                    match failed {
                        Some(e) => Err(e),
                        None => Ok(json!(seen)),
                    }
                }
                "save" => p.unwrap().save().map(|_| Value::Null),
                "balance" => p.unwrap().balance().map(|_| Value::Null),
                "short_push" => {
                    p.unwrap()
                        ._short_push(Shared::new(TimeValue::new(num()).into()));
                    Ok(Value::Null)
                }
                "long_push" => {
                    p.unwrap()
                        ._long_push(Shared::new(TimeValue::new(num()).into()));
                    Ok(Value::Null)
                }
                "remove_last" => p.unwrap()._remove_last().map(|_| Value::Null),
                other => panic!("{other}"),
            };
            let result = result.unwrap_or_else(|e| error(&e, base));
            steps.push(json!([result, state(&planner, base)]));
        }
        out.push(json!(steps));
    }
    json!(out)
}

/// PrusaLink's own `IsPlannerFed` and ours through the same operations: PrusaLink's tests
/// (`tests/test_is_planner_fed.py`) as operations, the threshold file in every form `float()`
/// takes or refuses, a missing folder, queues of 0, 1 and 2, broken heaps, and 25000 confirmation
/// times with `const.py`'s constants. After each: the same error and the same state (heaps with
/// `heap_value`, `heap_index` and heap name, the queue, the thresholds, the file's bytes).
#[test]
fn identical_to_prusalink() {
    let cases = cases();
    let base = std::env::temp_dir().join(format!("mc-planner-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let py_base = base.join("py");
    let rs_base = base.join("rs");
    std::fs::create_dir_all(&py_base).unwrap();
    let Some(theirs) = python::run(
        include_str!("../driver_is_planner_fed.py"),
        &json!({ "base": py_base.to_str().unwrap(), "cases": cases }),
    ) else {
        return;
    };
    let ours = ours(&cases, rs_base.to_str().unwrap());
    let _ = std::fs::remove_dir_all(&base);
    for (number, (their, our)) in theirs
        .as_array()
        .unwrap()
        .iter()
        .zip(ours.as_array().unwrap())
        .enumerate()
    {
        for (step, (t, o)) in their
            .as_array()
            .unwrap()
            .iter()
            .zip(our.as_array().unwrap())
            .enumerate()
        {
            assert_eq!(
                t, o,
                "case {number} step {step}: {}",
                cases[number][step][0]
            );
        }
        assert_eq!(their, our, "case {number}");
    }
    assert_eq!(theirs, ours);
}

/// PrusaLink's own `tests/test_is_planner_fed.py`, test by test.
mod prusalink_tests {
    use super::*;

    const QUEUE: usize = 10;

    /// `settings` (`:18-26`): Small queue, the dynamic threshold on, even heaps
    fn settings() -> Constants {
        Constants {
            queue_size: QUEUE,
            use_dynamic_threshold: true,
            default_threshold: 0.5,
            heap_ratio: 0.5,
            ignore_above: 10.0,
        }
    }

    struct TmpPath(std::path::PathBuf);

    impl Drop for TmpPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tmp_path(name: &str) -> TmpPath {
        let d = std::env::temp_dir().join(format!("mc-planner-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        TmpPath(d)
    }

    /// `planner` (`:29-32`): The planner over a not existing threshold file
    fn planner(tmp: &TmpPath) -> IsPlannerFed {
        let path = tmp.0.join("data").join("threshold.data");
        IsPlannerFed::with_constants(path.to_str().unwrap(), settings()).unwrap()
    }

    fn short_root(p: &IsPlannerFed) -> f64 {
        p.short_times.get(0).unwrap().lock().unwrap().item.value
    }

    fn long_root(p: &IsPlannerFed) -> f64 {
        p.long_times.get(0).unwrap().lock().unwrap().item.value
    }

    /// `test_defaults` (`:35-41`): Nothing known yet, the folder for the threshold is made
    #[test]
    fn test_defaults() {
        let tmp = tmp_path("defaults");
        let planner = planner(&tmp);
        assert_eq!(planner.default_threshold, 0.5);
        assert_eq!(planner.threshold().unwrap(), 0.5);
        assert!(!planner.call());
        assert_eq!(planner.item_count(), 0);
        assert!(tmp.0.join("data").is_dir());
    }

    /// `test_threshold_from_the_file` (`:44-50`): The saved threshold is used until enough values
    /// come
    #[test]
    fn test_threshold_from_the_file() {
        let tmp = tmp_path("file");
        let path = tmp.0.join("threshold.data");
        let p = path.to_str().unwrap();
        std::fs::write(&path, "0.25").unwrap();
        assert_eq!(
            IsPlannerFed::with_constants(p, settings())
                .unwrap()
                .default_threshold,
            0.25
        );
        std::fs::write(&path, "garbage").unwrap();
        assert_eq!(
            IsPlannerFed::with_constants(p, settings())
                .unwrap()
                .default_threshold,
            0.5
        );
    }

    /// `test_static_threshold` (`:53-59`): Without the dynamic threshold the default is always
    /// used
    #[test]
    fn test_static_threshold() {
        let tmp = tmp_path("static");
        let consts = Constants {
            use_dynamic_threshold: false,
            ..settings()
        };
        let mut planner =
            IsPlannerFed::with_constants(tmp.0.join("t").to_str().unwrap(), consts).unwrap();
        for _ in 0..QUEUE + 2 {
            planner.process_value(1.0).unwrap();
        }
        assert_eq!(planner.threshold().unwrap(), 0.5);
    }

    /// `test_dynamic_threshold_empty` (`:62-64`): No values, no threshold
    #[test]
    fn test_dynamic_threshold_empty() {
        let tmp = tmp_path("empty");
        assert_eq!(
            planner(&tmp).get_dynamic_threshold().unwrap(),
            f64::INFINITY
        );
    }

    /// `test_values_above_the_limit_are_ignored` (`:67-70`): Too long confirmations say nothing
    /// about the buffer
    #[test]
    fn test_values_above_the_limit_are_ignored() {
        let tmp = tmp_path("above");
        let mut planner = planner(&tmp);
        planner.process_value(11.0).unwrap();
        assert_eq!(planner.item_count(), 0);
    }

    /// `test_fed_decision` (`:73-80`): The long confirmation means the buffer is full
    #[test]
    fn test_fed_decision() {
        let tmp = tmp_path("fed");
        let mut planner = planner(&tmp);
        planner.process_value(0.1).unwrap();
        assert!(!planner.call());
        planner.process_value(1.0).unwrap();
        assert!(planner.call());
        planner.process_value(0.1).unwrap();
        assert!(!planner.call());
    }

    /// `test_first_values_fill_the_heaps` (`:83-91`): The first two values are put to the right
    /// heaps
    #[test]
    fn test_first_values_fill_the_heaps() {
        let tmp = tmp_path("first");
        let mut planner = planner(&tmp);
        planner.process_value(2.0).unwrap();
        let shorts: Vec<f64> = planner
            .short_times
            .heap
            .iter()
            .map(|i| i.lock().unwrap().item.value)
            .collect();
        assert_eq!(shorts, [2.0]);
        planner.process_value(1.0).unwrap();
        assert_eq!(short_root(&planner), 1.0);
        assert_eq!(long_root(&planner), 2.0);
        planner.process_value(3.0).unwrap();
        assert_eq!(planner.item_count(), 3);
    }

    /// `test_second_larger_value` (`:94-100`): A larger second value goes to the long heap
    #[test]
    fn test_second_larger_value() {
        let tmp = tmp_path("second");
        let mut planner = planner(&tmp);
        planner.process_value(1.0).unwrap();
        planner.process_value(2.0).unwrap();
        assert_eq!(long_root(&planner), 2.0);
        assert_eq!(planner.get_dynamic_threshold().unwrap(), 1.5);
        assert_eq!(planner.threshold().unwrap(), 0.5); // not enough values yet
    }

    /// `test_dynamic_threshold_after_filling_the_queue` (`:103-109`): With the full queue the
    /// percentile is used
    #[test]
    fn test_dynamic_threshold_after_filling_the_queue() {
        let tmp = tmp_path("filled");
        let mut planner = planner(&tmp);
        for value in 1..=QUEUE {
            planner.process_value(value as f64 / 10.0).unwrap();
        }
        assert_eq!(planner.item_count(), QUEUE);
        assert_eq!(
            planner.threshold().unwrap(),
            planner.get_dynamic_threshold().unwrap()
        );
        let threshold = planner.threshold().unwrap();
        assert!(0.1 < threshold && threshold <= 1.0);
    }

    /// `test_old_values_are_dropped` (`:112-117`): The queue and heaps stay limited
    #[test]
    fn test_old_values_are_dropped() {
        let tmp = tmp_path("dropped");
        let mut planner = planner(&tmp);
        for value in 1..4 * QUEUE {
            planner
                .process_value((value % 7 + 1) as f64 / 10.0)
                .unwrap();
        }
        assert_eq!(planner.item_count(), QUEUE);
        assert_eq!(planner.short_times.len() + planner.long_times.len(), QUEUE);
    }

    /// `test_heaps_stay_ordered` (`:120-126`): Every short value is not larger than every long
    /// one
    #[test]
    fn test_heaps_stay_ordered() {
        let tmp = tmp_path("ordered");
        let mut planner = planner(&tmp);
        for value in [
            5.0, 1.0, 9.0, 3.0, 7.0, 2.0, 8.0, 4.0, 6.0, 0.5, 9.5, 1.5, 2.5,
        ] {
            planner.process_value(value).unwrap();
            if !planner.long_times.is_empty() {
                assert!(short_root(&planner) <= long_root(&planner));
            }
        }
    }

    /// `test_balance_detects_broken_heaps` (`:129-134`): Broken ordering is an error
    #[test]
    fn test_balance_detects_broken_heaps() {
        let tmp = tmp_path("broken");
        let mut planner = planner(&tmp);
        planner._short_push(Shared::new(TimeValue::new(5.0).into()));
        planner._long_push(Shared::new(TimeValue::new(1.0).into()));
        assert_eq!(planner.balance().unwrap_err().class_name(), "RuntimeError");
    }

    /// `test_heap_names` (`:137-142`): Items remember their heap
    #[test]
    fn test_heap_names() {
        let item = TimeValue::new(1.0);
        assert_eq!(item.heap_name, None);
        let names: std::collections::HashSet<&str> = [HeapName::ShortTimes, HeapName::LongTimes]
            .into_iter()
            .map(HeapName::value)
            .collect();
        assert_eq!(names, ["SHORT_TIMES", "LONG_TIMES"].into());
    }

    /// `test_save_only_with_a_full_queue` (`:145-156`): The threshold is saved when it is reliable
    #[test]
    fn test_save_only_with_a_full_queue() {
        let tmp = tmp_path("save");
        let path = tmp.0.join("data").join("threshold.data");
        let mut planner = planner(&tmp);
        planner.process_value(1.0).unwrap();
        planner.save().unwrap();
        assert!(!path.exists());
        for value in 0..QUEUE {
            planner.process_value((value + 1) as f64 / 10.0).unwrap();
        }
        planner.save().unwrap();
        let read = || py_float(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read(), planner.get_dynamic_threshold().unwrap());
        assert_eq!(
            IsPlannerFed::with_constants(path.to_str().unwrap(), settings())
                .unwrap()
                .default_threshold,
            read()
        );
    }
}
