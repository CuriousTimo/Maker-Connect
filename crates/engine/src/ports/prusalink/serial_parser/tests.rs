use super::*;
use crate::ports::prusalink::python;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

/// `Mock()`: its calls, each with the match's groups.
#[derive(Default)]
struct Mock {
    calls: Mutex<Vec<Option<Arc<Match>>>>,
}

impl Mock {
    fn handler(self: &Arc<Self>) -> Handler {
        let me = self.clone();
        Arc::new(move |fired: &Fired| me.calls.lock().unwrap().push(fired.r#match.clone()))
    }
    fn count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
    fn last_group(&self, name: &str) -> String {
        let calls = self.calls.lock().unwrap();
        calls
            .last()
            .unwrap()
            .as_ref()
            .unwrap()
            .named(name)
            .unwrap()
            .to_owned()
    }
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap()
}

fn mock() -> (Arc<Mock>, Handler) {
    let m = Arc::new(Mock::default());
    let h = m.handler();
    (m, h)
}

// tests/test_serial_parser.py

/// `test_basic`: Basic, one handler, call it on match
#[test]
fn test_basic() {
    let regex = re(r"(?P<a>Hello)");
    let (handler, h) = mock();
    let parser = SerialParser::new();
    parser.add_handler(&regex, &h, 0.0).unwrap();
    parser.decide("Hello");
    assert_eq!(handler.count(), 1);
    assert_eq!(handler.last_group("a"), "Hello");
}

/// `test_inverted_basic`: Basic, don't call when it does not match
#[test]
fn test_inverted_basic() {
    let regex = re(r"(?P<a>Hello)");
    let (handler, h) = mock();
    let parser = SerialParser::new();
    parser.add_handler(&regex, &h, 0.0).unwrap();
    parser.decide("Bye");
    assert_eq!(handler.count(), 0);
}

/// `test_basic_removal`
#[test]
fn test_basic_removal() {
    let regex = re(r"(?P<a>Hello)");
    let (handler, h) = mock();
    let parser = SerialParser::new();
    parser.add_handler(&regex, &h, 0.0).unwrap();
    parser.remove_handler(&regex, &h).unwrap();
    parser.decide("Hello");
    assert_eq!(handler.count(), 0);
}

/// `test_priority`: Do call just the higher priority handler don't call the rest
#[test]
fn test_priority() {
    let (regex1, regex2, regex3) = (
        re(r"(?P<a>Hell[o])"),
        re(r"(?P<a>[H]ello)"),
        re(r"(?P<a>H[e]llo)"),
    );
    let ((handler1, h1), (handler2, h2), (handler3, h3)) = (mock(), mock(), mock());
    let parser = SerialParser::new();
    parser.add_handler(&regex1, &h1, 1.0).unwrap();
    parser.add_handler(&regex3, &h3, 3.0).unwrap();
    parser.add_handler(&regex2, &h2, 2.0).unwrap();
    parser.decide("Hello");
    assert_eq!(handler3.count(), 1);
    assert_eq!(handler2.count(), 0);
    assert_eq!(handler1.count(), 0);
    assert_eq!(handler3.last_group("a"), "Hello");
}

/// `test_bump_priority`: If the same regex is already registered, adding a handler with a higher
/// priority should elevate all other handlers to the same priority.
#[test]
fn test_bump_priority() {
    let (regex1, regex2) = (re(r"(?P<a>Hell[o])"), re(r"(?P<a>[H]ello)"));
    let ((handler1, h1), (handler2, h2), (handler3, h3)) = (mock(), mock(), mock());
    let parser = SerialParser::new();
    parser.add_handler(&regex1, &h1, 1.0).unwrap();
    parser.add_handler(&regex2, &h2, 2.0).unwrap();
    parser.add_handler(&regex1, &h3, 3.0).unwrap();
    parser.decide("Hello");
    assert_eq!(handler3.count(), 1);
    assert_eq!(handler1.count(), 1);
    assert_eq!(handler2.count(), 0);
    assert_eq!(handler3.last_group("a"), "Hello");
    assert_eq!(handler1.last_group("a"), "Hello");
}

// tests/test_serial_parser_handlers.py

const HELLO: &str = r"^hello (?P<who>\w+)$";
const ANY: &str = r"^.*$";

/// `test_pairing_description`: The description counts handlers
#[test]
fn test_pairing_description() {
    let pairing = RegexPairing::new(re(HELLO), 3.0);
    assert!(!pairing.str().contains("1 handler"));
    let first: Handler = Arc::new(|_: &Fired| {});
    pairing.signal.connect(&first, false);
    assert!(pairing.str().ends_with("having 1 handler"));
    let second: Handler = Arc::new(|_: &Fired| {});
    pairing.signal.connect(&second, false);
    assert!(pairing.str().ends_with("having 2 handlers"));
    assert!(pairing.str().contains("priority 3"));
}

/// `test_pairing_survives_failing_handler`: A broken handler must not kill the reader
#[test]
fn test_pairing_survives_failing_handler() {
    let regex = re(HELLO);
    let pairing = Arc::new(RegexPairing::new(regex.clone(), 0.0));
    let (_seen, seen) = mock();
    let broken: Handler = Arc::new(|_: &Fired| panic!("boom"));
    pairing.signal.connect(&broken, false);
    pairing.signal.connect(&seen, false);
    let captures = re_match(&regex, "hello you").unwrap();
    pairing.fire(Some(Arc::new(Match::new(&regex, &captures))));
}

/// `test_decide_calls_handler_with_match`: The handler gets the match
#[test]
fn test_decide_calls_handler_with_match() {
    let parser = SerialParser::new();
    let (handler, h) = mock();
    parser.add_handler(&re(HELLO), &h, 0.0).unwrap();
    parser.decide("hello world");
    assert_eq!(handler.last_group("who"), "world");
}

/// `test_decide_without_match`: Unknown lines are only logged
#[test]
fn test_decide_without_match() {
    let parser = SerialParser::new();
    let (handler, h) = mock();
    parser.add_handler(&re(HELLO), &h, 0.0).unwrap();
    parser.decide("bye");
    assert_eq!(handler.count(), 0);
}

/// `test_higher_priority_goes_first`: Only the first matching pairing is fired
#[test]
fn test_higher_priority_goes_first() {
    let parser = SerialParser::new();
    let ((low, l), (high, h)) = (mock(), mock());
    parser.add_handler(&re(ANY), &l, 1.0).unwrap();
    parser.add_handler(&re(HELLO), &h, 5.0).unwrap();
    parser.decide("hello you");
    assert_eq!(high.count(), 1);
    assert_eq!(low.count(), 0);
}

/// `test_same_regexp_collects_handlers`: All handlers of the one regexp are called
#[test]
fn test_same_regexp_collects_handlers() {
    let parser = SerialParser::new();
    let ((first, f), (second, s)) = (mock(), mock());
    parser.add_handler(&re(HELLO), &f, 0.0).unwrap();
    parser.add_handler(&re(HELLO), &s, 0.0).unwrap();
    parser.decide("hello you");
    assert_eq!(first.count(), 1);
    assert_eq!(second.count(), 1);
    assert_eq!(parser.pattern_list_len(), 1);
}

/// `test_adding_with_higher_priority_reorders`: A later handler can raise the priority of the
/// pairing
#[test]
fn test_adding_with_higher_priority_reorders() {
    let parser = SerialParser::new();
    let ((first, f), (second, s), (_third, t)) = (mock(), mock(), mock());
    parser.add_handler(&re(ANY), &f, 2.0).unwrap();
    parser.add_handler(&re(HELLO), &s, 1.0).unwrap();
    parser.add_handler(&re(HELLO), &t, 9.0).unwrap();
    parser.decide("hello you");
    assert_eq!(second.count(), 1);
    assert_eq!(first.count(), 0);
}

/// `test_remove_handler`: The pairing disappears with its last handler
#[test]
fn test_remove_handler() {
    let parser = SerialParser::new();
    let ((_first, f), (_second, s)) = (mock(), mock());
    parser.add_handler(&re(HELLO), &f, 0.0).unwrap();
    parser.add_handler(&re(HELLO), &s, 0.0).unwrap();
    parser.remove_handler(&re(HELLO), &f).unwrap();
    assert!(parser.has_pairing(&re(HELLO)));
    parser.remove_handler(&re(HELLO), &s).unwrap();
    assert!(!parser.has_pairing(&re(HELLO)));
    assert_eq!(parser.pattern_list_len(), 0);
}

/// `test_remove_unknown_handler`: Removing what was not added is an error
#[test]
fn test_remove_unknown_handler() {
    let parser = SerialParser::new();
    let (_m, h) = mock();
    let error = parser.remove_handler(&re(HELLO), &h).unwrap_err();
    assert_eq!(error.class_name(), "RuntimeError");
}

/// `test_decoupled_handler_runs_in_the_thread`: The handler is called from the decoupler thread
#[test]
fn test_decoupled_handler_runs_in_the_thread() {
    let threaded = ThreadedSerialParser::new().unwrap();
    let finished = Arc::new(crate::ports::python::py_threading::Event::new());
    let received: Arc<Mutex<Option<(std::thread::ThreadId, String)>>> = Default::default();
    let (f, r) = (finished.clone(), received.clone());
    let handler: Handler = Arc::new(move |fired: &Fired| {
        let who = fired
            .r#match
            .as_ref()
            .unwrap()
            .named("who")
            .unwrap()
            .to_owned();
        *r.lock().unwrap() = Some((std::thread::current().id(), who));
        f.set();
    });
    threaded
        .add_decoupled_handler(&re(HELLO), &handler, 0.0)
        .unwrap();
    threaded.decide("hello world");
    assert!(finished.wait(Some(2.0)));
    let (thread, who) = received.lock().unwrap().clone().unwrap();
    assert_ne!(thread, std::thread::current().id());
    assert_eq!(who, "world");
    threaded.stop();
    threaded.wait_stopped().unwrap();
}

/// `test_stop_ends_the_thread`: Stopping wakes the thread up
#[test]
fn test_stop_ends_the_thread() {
    let threaded = ThreadedSerialParser::new().unwrap();
    threaded.stop();
    threaded.wait_stopped().unwrap();
    assert!(!threaded.thread.is_alive());
}

// Against PrusaLink's own SerialParser

const PATTERNS: [&str; 5] = [
    r"^ok\b",
    r"(?P<a>Hell[o])",
    r"^T:(?P<t>\d+\.\d+)",
    r".*",
    r"^echo:(?P<e>.*)$",
];
const PRIORITIES: [f64; 6] = [0.0, 1.0, 2.5, f64::INFINITY, 1764270000.25, 2.0];
const LINES: [&str; 7] = [
    "ok",
    "ok T:20.0",
    "Hello",
    "say Hello",
    "T:21.5 /0.0",
    "echo:busy",
    "nothing",
];

fn ops() -> Vec<Value> {
    let mut seed: u64 = 2431;
    let mut next = |n: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % n
    };
    let mut ops = vec![];
    for _ in 0..500 {
        let p = next(PATTERNS.len() as u64);
        let h = next(8);
        match next(10) {
            0..=3 => {
                let priority = PRIORITIES[next(PRIORITIES.len() as u64) as usize];
                let priority = if priority.is_infinite() {
                    json!("inf")
                } else {
                    json!(priority)
                };
                ops.push(json!(["add", p, h, priority]));
            }
            4..=5 => ops.push(json!(["remove", p, h])),
            _ => ops.push(json!(["decide", LINES[next(LINES.len() as u64) as usize]])),
        }
    }
    ops
}

/// PrusaLink's `SerialParser` and ours through the same 500 operations (adds with equal,
/// raised, lowered and infinite priorities, the same handler twice, removals of what is there
/// and not, lines that match one pattern, several or none): after each, the same result (the
/// `RuntimeError`'s text), the handlers called (sorted: blinker's order) with the same groups,
/// and `pattern_list` in the same order with the same `str` of each pairing.
#[test]
fn identical_to_prusalink() {
    let ops = ops();
    let Some(theirs) = python::run(
        include_str!("../driver_serial_parser.py"),
        &json!({"patterns": PATTERNS, "ops": ops}),
    ) else {
        return;
    };
    // a pairing on its own: no handler, one, two; each priority
    let mut strs = vec![];
    for priority in PRIORITIES {
        let pairing = RegexPairing::new(re(PATTERNS[1]), priority);
        strs.push(pairing.str());
        for _ in 0..2 {
            let h: Handler = Arc::new(|_: &Fired| {});
            pairing.signal.connect(&h, false);
            strs.push(pairing.str());
        }
    }
    assert_eq!(json!(strs), theirs["strs"]);
    let theirs = theirs["results"].as_array().unwrap();
    assert_eq!(theirs.len(), ops.len());
    let parser = SerialParser::new();
    let regexes: Vec<Regex> = PATTERNS.iter().map(|p| re(p)).collect();
    let calls: Arc<Mutex<Vec<(usize, Vec<Option<String>>)>>> = Default::default();
    let handlers: Vec<Handler> = (0..8)
        .map(|i| {
            let calls = calls.clone();
            let h: Handler = Arc::new(move |fired: &Fired| {
                let groups = fired.r#match.as_ref().unwrap().groups.clone();
                calls.lock().unwrap().push((i, groups));
            });
            h
        })
        .collect();
    let decided = AtomicUsize::new(0);
    for (op, their) in ops.iter().zip(theirs) {
        let result = match op[0].as_str().unwrap() {
            "add" => {
                let priority = op[3].as_f64().unwrap_or(f64::INFINITY);
                let p = op[1].as_u64().unwrap() as usize;
                let h = op[2].as_u64().unwrap() as usize;
                json!(
                    parser
                        .add_handler(&regexes[p], &handlers[h], priority)
                        .map_err(|e| [e.class_name().to_owned(), e.message().to_owned()])
                        .err()
                )
            }
            "remove" => {
                let p = op[1].as_u64().unwrap() as usize;
                let h = op[2].as_u64().unwrap() as usize;
                json!(
                    parser
                        .remove_handler(&regexes[p], &handlers[h])
                        .map_err(|e| [e.class_name().to_owned(), e.message().to_owned()])
                        .err()
                )
            }
            _ => {
                calls.lock().unwrap().clear();
                parser.decide(op[1].as_str().unwrap());
                decided.fetch_add(1, Ordering::SeqCst);
                let mut got = calls.lock().unwrap().clone();
                got.sort();
                json!(got)
            }
        };
        let ours = json!([result, parser.pairings()]);
        assert_eq!(
            serde_json::to_string(&ours).unwrap(),
            serde_json::to_string(their).unwrap(),
            "{op}"
        );
    }
    assert!(decided.load(Ordering::SeqCst) > 100);
}
