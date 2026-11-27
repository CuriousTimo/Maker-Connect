use super::*;
use crate::ports::prusalink::python;
use serde_json::json;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counter {
    calls: AtomicUsize,
    quit_evt: Arc<Event>,
}

impl Update for Counter {
    fn update_interval(&self) -> f64 {
        0.001
    }
    fn update(&self) {
        if self.calls.fetch_add(1, Ordering::SeqCst) + 1 == 3 {
            self.quit_evt.set();
        }
    }
}

struct Base;
impl Update for Base {
    fn update(&self) {}
}

/// `loop_until` and `ThreadedUpdatable`, PrusaLink's and ours: the same calls in the same order
/// until the event is set; an updatable that stops itself at its third update has run three
/// times and its thread has ended; `update_interval` and `thread_name` the same. The base
/// `update`'s `NotImplementedError` is a trait method without a default here.
#[test]
fn identical_to_prusalink() {
    let Some(theirs) = python::run(include_str!("../driver_updatable.py"), &json!({})) else {
        return;
    };
    let order = Mutex::new(vec![]);
    let evt = Event::new();
    loop_until(
        &evt,
        || {
            order.lock().unwrap().push("every");
            0.0
        },
        || {
            let mut o = order.lock().unwrap();
            o.push("run");
            if o.iter().filter(|s| **s == "run").count() == 3 {
                evt.set();
            }
        },
    );
    assert_eq!(json!(*order.lock().unwrap()), theirs["order"]);

    let updatable = ThreadedUpdatable::new("counter");
    let counter = Arc::new(Counter {
        calls: AtomicUsize::new(0),
        quit_evt: updatable.quit_evt.clone(),
    });
    updatable.start(counter.clone()).unwrap();
    updatable.wait_stopped().unwrap();
    assert_eq!(json!(counter.calls.load(Ordering::SeqCst)), theirs["calls"]);
    assert_eq!(json!(updatable.thread.is_alive()), theirs["alive"]);
    assert_eq!(
        json!([ThreadedUpdatable::THREAD_NAME, updatable.thread_name]),
        theirs["name"]
    );
    assert_eq!(json!(Base.update_interval()), theirs["interval"]);
    assert_eq!(theirs["base"], "NotImplementedError");
}
