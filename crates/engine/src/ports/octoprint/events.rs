//! Port of the parts of OctoPrint 1.11.8 `src/octoprint/events.py` the comm layer and
//! `printer/standard.py` use: the `Events` name constants (`events.py:29`, only those `util/comm.py`,
//! `printer/standard.py` and the prompt plugin refer to, and `STARTUP` / `SHUTDOWN`), `Events._to_identifier`
//! (:149) and the `EventManager` dispatcher (:160-251): `__init__` (:164, the worker is a daemon thread
//! started with the manager), `_work` (:181, with its `SHUTDOWN` handling and the outer `except` "the event
//! bus worker loop crashed"), `fire` (:215, with `_startup_signaled` and the events held back until `STARTUP`
//! and their move to the queue), `_enqueue` (:238), `subscribe` (:253) and `unsubscribe` (:265).
//!
//! Without `STARTUP` nothing is delivered (:238-242: until then `_enqueue` holds events back). The port's
//! top edge MUST fire [`Events::STARTUP`] once when it starts, and `Events::SHUTDOWN` when it ends (the
//! worker leaves its loop after it). Under `cfg(test)` the global manager fires `STARTUP` when it is made.
//!
//! OURS: the listeners of `octoprint.plugin.call_plugin(EventHandlerPlugin, "on_event", ...)` (:206) are
//! registered by the plugin with [`EventManager::register_event_handler_plugin`] (there is no plugin
//! loading), and called in registration order after the listeners of the event, each in `py_try` (as
//! `call_plugin` logs and goes on). The [`EventSink`] the top edge installs with
//! [`EventManager::set_sink`] is called last, in the worker thread, inside `py_try` (it is not part of
//! `events.py`: it is how the port's top edge receives the events). Logging is not ported (`self._logger`
//! calls are comments). Not ported (not wire): `CommandTrigger` and its `GcodeEventSubscription` /
//! `SystemEventSubscription` (:325-572, they act only on `events.subscriptions` of the settings, empty
//! by default), `GenericEventListener`, `DebugEventListener`, `all_events`, `register_event`.

use super::comm::py_try;
use crate::ports::python::py_queue::{Deque, GetError, Queue};
use crate::ports::python::py_threading::{LockExt, Thread};
use fancy_regex::Regex;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

/// `events.py:29 Events`: the names `util/comm.py` uses, with their values.
pub struct Events;

impl Events {
    pub const STARTUP: &'static str = "Startup"; // :31
    pub const SHUTDOWN: &'static str = "Shutdown"; // :32
    pub const PRINTER_RESET: &'static str = "PrinterReset"; // :44
    pub const CONNECTING: &'static str = "Connecting"; // :36
    pub const CONNECTED: &'static str = "Connected"; // :37
    pub const DISCONNECTING: &'static str = "Disconnecting"; // :38
    pub const DISCONNECTED: &'static str = "Disconnected"; // :39
    pub const PRINTER_STATE_CHANGED: &'static str = "PrinterStateChanged"; // :43
    pub const FILE_SELECTED: &'static str = "FileSelected"; // :58
    pub const FILE_DESELECTED: &'static str = "FileDeselected"; // :59
    pub const UPDATED_FILES: &'static str = "UpdatedFiles"; // :60
    pub const TRANSFER_STARTED: &'static str = "TransferStarted"; // :73
    pub const TRANSFER_DONE: &'static str = "TransferDone"; // :74
    pub const TRANSFER_FAILED: &'static str = "TransferFailed"; // :75
    pub const PRINT_STARTED: &'static str = "PrintStarted"; // :78
    pub const PRINT_DONE: &'static str = "PrintDone"; // :79
    pub const PRINT_FAILED: &'static str = "PrintFailed"; // :80
    pub const PRINT_CANCELLING: &'static str = "PrintCancelling"; // :81
    pub const PRINT_CANCELLED: &'static str = "PrintCancelled"; // :82
    pub const PRINT_PAUSED: &'static str = "PrintPaused"; // :83
    pub const PRINT_RESUMED: &'static str = "PrintResumed"; // :84
    pub const CHART_MARKED: &'static str = "ChartMarked"; // :86
    pub const Z_CHANGE: &'static str = "ZChange"; // :92
    pub const POSITION_UPDATE: &'static str = "PositionUpdate"; // :100
    pub const ERROR: &'static str = "Error"; // :85
    pub const POWER_ON: &'static str = "PowerOn"; // :89
    pub const POWER_OFF: &'static str = "PowerOff"; // :90
    pub const HOME: &'static str = "Home"; // :91
    pub const WAITING: &'static str = "Waiting"; // :93
    pub const DWELL: &'static str = "Dwelling"; // :94
    pub const COOLING: &'static str = "Cooling"; // :95
    pub const ALERT: &'static str = "Alert"; // :96
    pub const CONVEYOR: &'static str = "Conveyor"; // :97
    pub const EJECT: &'static str = "Eject"; // :98
    pub const E_STOP: &'static str = "EStop"; // :99
    pub const FIRMWARE_DATA: &'static str = "FirmwareData"; // :101
    pub const TOOL_CHANGE: &'static str = "ToolChange"; // :102
    pub const REGISTERED_MESSAGE_RECEIVED: &'static str = "RegisteredMessageReceived"; // :103
    pub const COMMAND_SUPPRESSED: &'static str = "CommandSuppressed"; // :104
    pub const INVALID_TOOL_REPORTED: &'static str = "InvalidToolReported"; // :105
    pub const FILAMENT_CHANGE: &'static str = "FilamentChange"; // :106
}

impl Events {
    /// `events.py:149 _to_identifier(name)`, with `_first_cap_re` (:145) and `_all_cap_re` (:146)
    pub fn _to_identifier(name: &str) -> String {
        static FIRST_CAP_RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"([^_])([A-Z][a-z]+)").expect("valid regex"));
        static ALL_CAP_RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"([a-z0-9])([A-Z])").expect("valid regex"));
        let s1 = FIRST_CAP_RE.replace_all(name, r"${1}_${2}");
        ALL_CAP_RE.replace_all(&s1, r"${1}_${2}").to_uppercase()
    }
}

/// OURS: where `fire` delivers (see the module doc).
pub trait EventSink: Send + Sync {
    fn fire(&self, event: &str, payload: Option<Value>);
}

/// A listener, `callback(event, payload)` (`events.py:253`), or the `on_event(event, payload)` of an
/// event-handler plugin.
pub type Listener = Arc<dyn Fn(&str, Option<&Value>) + Send + Sync>;

/// `(event, payload)`, what `fire` puts on the queues
type QueuedEvent = (String, Option<Value>);

/// `events.py:160 EventManager`: 'Handles receiving events and dispatching them to subscribers'
pub struct EventManager {
    sink: Mutex<Option<Arc<dyn EventSink>>>,
    /// `self._registeredListeners`: event name -> listeners, in subscription order
    registered_listeners: Mutex<BTreeMap<String, Vec<Listener>>>,
    /// OURS: the `on_event` of the `EventHandlerPlugin` implementations (`call_plugin`, :206)
    event_handler_plugins: Mutex<Vec<Listener>>,
    /// `self._startup_signaled`
    startup_signaled: AtomicBool,
    /// `self._shutdown_signaled`
    shutdown_signaled: AtomicBool,
    /// `self._queue`
    queue: Queue<Deque<QueuedEvent>>,
    /// `self._held_back`
    held_back: Queue<Deque<QueuedEvent>>,
    /// `self._worker`
    worker: Thread,
    /// OURS (tests only): the events put on a queue and not yet delivered, for [`EventManager::wait_idle`]
    #[cfg(test)]
    pending: std::sync::atomic::AtomicI64,
}

static INSTANCE: OnceLock<Arc<EventManager>> = OnceLock::new();

/// `events.py:154 eventManager()`
pub fn event_manager() -> &'static EventManager {
    INSTANCE.get_or_init(|| {
        let manager = EventManager::new();
        // OURS: the top edge fires STARTUP (module doc); the tests have no top edge
        #[cfg(test)]
        manager.fire(Events::STARTUP, None);
        manager
    })
}

impl EventManager {
    /// `events.py:164 __init__`: the worker is a daemon thread, started with the manager. The worker
    /// holds the manager as long as it runs (it is `self._work`); a manager is ended with `SHUTDOWN`.
    pub fn new() -> Arc<EventManager> {
        let manager = Arc::new(EventManager {
            sink: Mutex::new(None),
            registered_listeners: Mutex::new(BTreeMap::new()),
            event_handler_plugins: Mutex::new(Vec::new()),
            startup_signaled: AtomicBool::new(false),
            shutdown_signaled: AtomicBool::new(false),
            queue: Queue::new(0, Deque::new()),
            held_back: Queue::new(0, Deque::new()),
            worker: Thread::new(true),
            #[cfg(test)]
            pending: std::sync::atomic::AtomicI64::new(0),
        });
        let me = manager.clone();
        manager
            .worker
            .start(move || me._work())
            .expect("the worker starts once");
        manager
    }

    /// `events.py:181 _work`
    fn _work(&self) {
        // try: ... except Exception: self._logger.exception("Ooops, the event bus worker loop crashed")
        let _ = py_try(|| {
            while !self.shutdown_signaled.load(Ordering::SeqCst) {
                let (event, payload) = match self.queue.get(true, None) {
                    Ok(item) => item,
                    Err(GetError::Empty)
                    | Err(GetError::ShutDown)
                    | Err(GetError::ValueError(_)) => {
                        panic!("Exception: the event queue raised")
                    }
                };
                if event == Events::SHUTDOWN {
                    // we've got the shutdown event here, stop event loop processing after this has been processed
                    // self._logger.info("Processing shutdown event, this will be our last event")
                    self.shutdown_signaled.store(true, Ordering::SeqCst);
                }

                let listeners: Vec<Listener> = self
                    .registered_listeners
                    .plock()
                    .get(&event)
                    .cloned()
                    .unwrap_or_default();
                // self._logger_fire.debug(f"Firing event: {event} (Payload: {payload!r})")

                for listener in listeners {
                    // self._logger.debug(f"Sending action to {listener!r}")
                    // except Exception: self._logger.exception("Got an exception while sending event ...")
                    let _ = py_try(|| listener(&event, payload.as_ref()));
                }

                // octoprint.plugin.call_plugin(EventHandlerPlugin, "on_event", args=(event, payload)):
                // it catches the exception of each plugin
                let plugins: Vec<Listener> = self.event_handler_plugins.plock().clone();
                for on_event in plugins {
                    let _ = py_try(|| on_event(&event, payload.as_ref()));
                }

                // OURS: the sink of the top edge, after the plugins, in this thread, inside py_try
                let sink = self.sink.plock().clone();
                if let Some(sink) = sink {
                    let _ = py_try(|| sink.fire(&event, payload));
                }

                #[cfg(test)]
                self.pending.fetch_sub(1, Ordering::SeqCst);
            }
            // self._logger.info("Event loop shut down")
        });
    }

    /// OURS: install the sink.
    pub fn set_sink(&self, sink: Arc<dyn EventSink>) {
        *self.sink.plock() = Some(sink);
    }

    /// OURS: register the `on_event` of an `EventHandlerPlugin` implementation
    pub fn register_event_handler_plugin(&self, on_event: Listener) {
        self.event_handler_plugins.plock().push(on_event);
    }

    /// `events.py:215 fire(event, payload=None)`: 'Fire an event to anyone subscribed to it'
    pub fn fire(&self, event: &str, payload: Option<Value>) {
        let mut send_held_back = false;
        if event == Events::STARTUP {
            // self._logger.info("Processing startup event, this is our first event")
            self.startup_signaled.store(true, Ordering::SeqCst);
            send_held_back = true;
        }

        self._enqueue(event, payload);

        if send_held_back {
            // self._logger.info("Adding {} events to queue that were held back before startup event")
            while let Ok(item) = self.held_back.get(false, None) {
                self.queue
                    .put(item, true, None)
                    .expect("an unbounded queue accepts the item");
            }
        }
    }

    /// `events.py:238 _enqueue`
    fn _enqueue(&self, event: &str, payload: Option<Value>) {
        let q = if self.startup_signaled.load(Ordering::SeqCst) {
            &self.queue
        } else {
            &self.held_back
        };
        #[cfg(test)]
        self.pending.fetch_add(1, Ordering::SeqCst);
        q.put((event.to_string(), payload), true, None)
            .expect("an unbounded queue accepts the item");
    }

    /// `events.py:253 subscribe(event, callback)`: 'Subscribe a listener to an event -- pass in the event
    /// name (as a string) and the callback object'
    pub fn subscribe(&self, event: &str, callback: Listener) {
        let mut listeners = self.registered_listeners.plock();
        let list = listeners.entry(event.to_string()).or_default();
        if list.iter().any(|l| Arc::ptr_eq(l, &callback)) {
            // callback is already subscribed to the event
            return;
        }
        list.push(callback);
    }

    /// `events.py:265 unsubscribe(event, callback)`
    pub fn unsubscribe(&self, event: &str, callback: &Listener) {
        let mut listeners = self.registered_listeners.plock();
        if let Some(list) = listeners.get_mut(event) {
            if let Some(i) = list.iter().position(|l| Arc::ptr_eq(l, callback)) {
                list.remove(i);
            }
        }
    }

    /// OURS (tests only): wait, at most 10 s, until every event fired so far has been delivered to the
    /// listeners, the plugins and the sink. `_work` never calls `task_done()`, so `Queue.join()` is not
    /// possible; [`EventManager::pending`] counts instead. A timeout panics (the worker is stuck or dead).
    #[cfg(test)]
    pub(crate) fn wait_idle(&self) {
        let start = std::time::Instant::now();
        while self.pending.load(Ordering::SeqCst) > 0 {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(10),
                "the event worker did not deliver the events in 10 s"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

/// OURS (tests only): [`EventManager::wait_idle`] of the global manager
#[cfg(test)]
pub(crate) fn wait_idle() {
    event_manager().wait_idle();
}

#[cfg(test)]
mod tests {
    //! `tests/test_events.py` (:14-27 `test_to_identifier`) ported. `test_escape_system_commands` and
    //! `test_handle_system_commands` test `SystemEventSubscription` (:520-570), which runs a shell
    //! command a user configured under `events.subscriptions`; it is not ported (see the module doc),
    //! so those two do not apply. The rest are OURS (the reference has no test of the worker).
    use super::*;

    #[test]
    fn test_to_identifier() {
        for (value, expected) in [
            ("plugin_example_event", "PLUGIN_EXAMPLE_EVENT"),
            ("plugin_Example_event", "PLUGIN_EXAMPLE_EVENT"),
            ("plugin_ExAmple_Event", "PLUGIN_EX_AMPLE_EVENT"),
            ("plugin_exAmple_EvEnt", "PLUGIN_EX_AMPLE_EV_ENT"),
        ] {
            assert_eq!(expected, Events::_to_identifier(value));
        }
    }

    use serde_json::json;
    use std::sync::atomic::AtomicI64;

    /// a local manager, started (`STARTUP` fired) unless a test wants it held back
    fn started() -> Arc<EventManager> {
        let m = EventManager::new();
        m.fire(Events::STARTUP, None);
        m
    }

    /// a local manager is ended with `SHUTDOWN`: the worker leaves its loop and the thread ends
    fn shutdown(m: &Arc<EventManager>) {
        m.fire(Events::SHUTDOWN, None);
        m.worker.join(Some(10.0)).expect("started");
        assert!(!m.worker.is_alive(), "the worker ends after SHUTDOWN");
    }

    fn recorder(
        m: &EventManager,
        events: &[&str],
    ) -> Arc<Mutex<Vec<(String, Option<Value>, std::thread::ThreadId)>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        for event in events {
            let seen = seen.clone();
            let listener: Listener = Arc::new(move |e, p| {
                seen.lock()
                    .unwrap()
                    .push((e.to_string(), p.cloned(), std::thread::current().id()))
            });
            m.subscribe(event, listener);
        }
        seen
    }

    #[test]
    fn ours_subscribe_fire_unsubscribe() {
        let m = started();
        let seen = Arc::new(AtomicI64::new(0));
        let s2 = seen.clone();
        let l: Listener = Arc::new(move |event, payload| {
            assert_eq!("Test", event);
            s2.fetch_add(
                payload.and_then(Value::as_i64).unwrap_or(1),
                Ordering::SeqCst,
            );
        });
        m.subscribe("Test", l.clone());
        m.subscribe("Test", l.clone()); // already subscribed (:256)
        m.fire("Test", Some(json!(5)));
        m.fire("Other", None);
        m.wait_idle();
        assert_eq!(5, seen.load(Ordering::SeqCst));
        m.unsubscribe("Test", &l);
        m.fire("Test", None);
        m.wait_idle();
        assert_eq!(5, seen.load(Ordering::SeqCst));
        // a listener that raises does not stop the next (`except Exception`, :199)
        m.subscribe("Test", Arc::new(|_, _| panic!("Exception: boom")));
        m.subscribe("Test", l);
        m.fire("Test", Some(json!(2)));
        m.wait_idle();
        assert_eq!(7, seen.load(Ordering::SeqCst));
        shutdown(&m);
    }

    /// `__init__` (:164-179): the worker is a daemon thread started with the manager; `fire` returns at
    /// once and the listeners run in the worker (:181-213), not in the caller's thread
    #[test]
    fn ours_listeners_run_in_the_worker_thread() {
        let m = started();
        assert!(m.worker.daemon);
        assert!(m.worker.is_alive());
        let seen = recorder(&m, &["Threaded"]);
        m.fire("Threaded", None);
        m.wait_idle();
        let seen = seen.lock().unwrap();
        assert_eq!(1, seen.len());
        assert_ne!(std::thread::current().id(), seen[0].2);
        drop(seen);
        shutdown(&m);
    }

    /// `fire` and `_enqueue` (:215-242): before `STARTUP` the events are held back; `STARTUP` is first,
    /// then the held back events in the order they were fired
    #[test]
    fn ours_events_are_held_back_until_startup() {
        let m = EventManager::new();
        let seen = recorder(&m, &[Events::STARTUP, "A", "B"]);
        m.fire("A", Some(json!(1)));
        m.fire("B", Some(json!(2)));
        // nothing is delivered without STARTUP: wait a moment (the worker would have had time) and look
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(seen.lock().unwrap().is_empty());
        assert_eq!(2, m.held_back.qsize());
        assert_eq!(0, m.queue.qsize());

        m.fire(Events::STARTUP, None);
        m.wait_idle();
        let names: Vec<(String, Option<Value>)> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|(e, p, _)| (e.clone(), p.clone()))
            .collect();
        assert_eq!(
            vec![
                ("Startup".to_string(), None),
                ("A".to_string(), Some(json!(1))),
                ("B".to_string(), Some(json!(2)))
            ],
            names
        );
        assert_eq!(0, m.held_back.qsize());

        // after STARTUP the events go to the queue
        m.fire("A", Some(json!(3)));
        m.wait_idle();
        assert_eq!(4, seen.lock().unwrap().len());
        shutdown(&m);
    }

    /// `_work` (:181-213): `SHUTDOWN` is delivered, and it is the last event
    #[test]
    fn ours_shutdown_is_the_last_event() {
        let m = started();
        let seen = recorder(&m, &[Events::SHUTDOWN, "After"]);
        m.fire(Events::SHUTDOWN, None);
        m.fire("After", None);
        m.worker.join(Some(10.0)).expect("started");
        assert!(!m.worker.is_alive());
        let names: Vec<String> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|(e, _, _)| e.clone())
            .collect();
        assert_eq!(vec!["Shutdown".to_string()], names);
        assert!(m.shutdown_signaled.load(Ordering::SeqCst));
        assert_eq!(1, m.queue.qsize());
    }

    /// the listeners, then the event handler plugins (:206), then the sink (OURS): in that order, each
    /// guarded, in the worker thread
    #[test]
    fn ours_listeners_then_plugins_then_sink() {
        struct Sink(Arc<Mutex<Vec<String>>>);
        impl EventSink for Sink {
            fn fire(&self, event: &str, _payload: Option<Value>) {
                self.0.lock().unwrap().push(format!("sink {event}"));
                if event == "Boom" {
                    panic!("Exception: the sink raises");
                }
            }
        }
        let m = started();
        m.wait_idle(); // STARTUP is delivered before the plugin and the sink are registered
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let l = log.clone();
        m.subscribe(
            "Order",
            Arc::new(move |e, _| l.lock().unwrap().push(format!("listener {e}"))),
        );
        let l = log.clone();
        m.register_event_handler_plugin(Arc::new(move |e, _| {
            l.lock().unwrap().push(format!("plugin {e}"));
            if e == "Boom" {
                panic!("Exception: the plugin raises");
            }
        }));
        m.set_sink(Arc::new(Sink(log.clone())));
        m.fire("Order", None);
        m.fire("Boom", None);
        m.fire("Order", None); // the worker is alive after a plugin and a sink that raised
        m.wait_idle();
        assert_eq!(
            vec![
                "listener Order",
                "plugin Order",
                "sink Order",
                "plugin Boom",
                "sink Boom",
                "listener Order",
                "plugin Order",
                "sink Order"
            ],
            log.lock()
                .unwrap()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        shutdown(&m);
    }

    /// the global manager has `STARTUP` fired under `cfg(test)`; the names are unique to this test (the
    /// other tests share the manager, so nothing else is asserted about it)
    #[test]
    fn ours_global_manager_delivers() {
        let seen = recorder(event_manager(), &["EventsTestGlobalManager"]);
        event_manager().fire("EventsTestGlobalManager", Some(json!("x")));
        wait_idle();
        assert_eq!(1, seen.lock().unwrap().len());
        assert!(event_manager().startup_signaled.load(Ordering::SeqCst));
    }
}
