//! SDK Exceptions (`errors.py`): deprecated, use [`crate::conditions`] instead. `Printer` still
//! sets this chain next to the conditions, so it is ported too.
//!
//! An [`ErrorState`] is a handle into one process-wide store, as the Python objects are
//! module globals. Python takes no lock here; the store's mutex only guards memory, and the
//! callbacks run with it released.

use std::fmt;
use std::sync::{Arc, LazyLock, Mutex};

/// `detected_cb` / `resolved_cb`: called with the old value.
pub type ErrorCallback = Arc<dyn Fn(Option<bool>) + Send + Sync>;

struct Node {
    name: String,
    long_msg: String,
    short_msg: String,
    prev: Option<usize>,
    next: Option<usize>,
    ok: Option<bool>,
    detected_cb: ErrorCallback,
    resolved_cb: ErrorCallback,
}

static STORE: Mutex<Vec<Node>> = Mutex::new(Vec::new());

fn store() -> std::sync::MutexGuard<'static, Vec<Node>> {
    STORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Model chained error states as doubly linked list
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ErrorState(usize);

impl ErrorState {
    pub fn new(
        name: &str,
        long_msg: &str,
        prev: Option<ErrorState>,
        short_msg: Option<&str>,
    ) -> Self {
        let mut nodes = store();
        let index = nodes.len();
        let short_msg = short_msg.filter(|s| !s.is_empty()).unwrap_or(name);
        nodes.push(Node {
            name: name.to_owned(),
            long_msg: long_msg.to_owned(),
            short_msg: short_msg.to_owned(),
            prev: None,
            next: None,
            ok: None, // None = unknown, True = OK, False = NOK
            detected_cb: Arc::new(|_| {}),
            resolved_cb: Arc::new(|_| {}),
        });
        drop(nodes);
        let state = ErrorState(index);
        state.set_prev(prev);
        state
    }

    pub fn name(self) -> String {
        store()[self.0].name.clone()
    }

    pub fn long_msg(self) -> String {
        store()[self.0].long_msg.clone()
    }

    pub fn short_msg(self) -> String {
        store()[self.0].short_msg.clone()
    }

    pub fn next(self) -> Option<ErrorState> {
        store()[self.0].next.map(ErrorState)
    }

    /// Returns previous state in the state chain.
    pub fn prev(self) -> Option<ErrorState> {
        store()[self.0].prev.map(ErrorState)
    }

    /// The setter will also set `prev.next` to `self`.
    pub fn set_prev(self, prev: Option<ErrorState>) {
        let mut nodes = store();
        nodes[self.0].prev = prev.map(|p| p.0);
        if let Some(prev) = prev {
            nodes[prev.0].next = Some(self.0);
        }
    }

    pub fn set_detected_cb(self, cb: ErrorCallback) {
        store()[self.0].detected_cb = cb;
    }

    pub fn set_resolved_cb(self, cb: ErrorCallback) {
        store()[self.0].resolved_cb = cb;
    }

    /// Returns True if current state is OK, False if not, None if unknown
    pub fn ok(self) -> Option<bool> {
        store()[self.0].ok
    }

    /// When setting, new `value` is propagated back (if it is True) or forward (if False). For
    /// performance's sake this happens only if the current value is different from new.
    pub fn set_ok(self, value: bool) {
        let (old_value, prev, next, detected_cb, resolved_cb) = {
            let mut nodes = store();
            let node = &mut nodes[self.0];
            if node.ok == Some(value) {
                // skip updating prev/next if there is no change
                return;
            }
            let old_value = node.ok;
            node.ok = Some(value);
            (
                old_value,
                node.prev,
                node.next,
                node.detected_cb.clone(),
                node.resolved_cb.clone(),
            )
        };
        if value {
            resolved_cb(old_value);
            if let Some(prev) = prev {
                ErrorState(prev).set_ok(value);
            }
        } else {
            detected_cb(old_value);
            if let Some(next) = next {
                ErrorState(next).set_ok(value);
            }
        }
    }

    /// Shorthand for `self.ok`. NOTE that this returns False if `self.ok` is None
    pub fn is_ok(self) -> bool {
        self.ok() == Some(true)
    }

    /// `__iter__`: this state and every one after it.
    pub fn iter(self) -> Vec<ErrorState> {
        let mut items = vec![self];
        let mut item = self;
        while let Some(next) = item.next() {
            items.push(next);
            item = next;
        }
        items
    }
}

impl fmt::Display for ErrorState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ok = match self.ok() {
            None => "None",
            Some(true) => "True",
            Some(false) => "False",
        };
        write!(f, "{}: {ok}", self.name())
    }
}

/// The module's chain, created in order on first use.
pub struct Chain {
    pub internet: ErrorState,
    pub http: ErrorState,
    pub token: ErrorState,
    pub api: ErrorState,
}

/// Error chain representing a kind of semaphore signaling the status of the connection to
/// Connect
pub static CHAIN: LazyLock<Chain> = LazyLock::new(|| {
    let internet = ErrorState::new(
        "Internet",
        "DNS does not work, or there are other problems in communication to other hosts in the \
         Internet.",
        None,
        None,
    );
    let http = ErrorState::new(
        "HTTP",
        "HTTP communication to Connect fails, there are 5XX statuses",
        Some(internet),
        None,
    );
    // Signal if we have a token or not
    let token = ErrorState::new(
        "Token",
        "Printer has no valid token, it needs to be registered with Connect.",
        Some(http),
        None,
    );
    let api = ErrorState::new(
        "API",
        "Encountered 4XX problems while communicating to Connect",
        Some(token),
        None,
    );
    Chain {
        internet,
        http,
        token,
        api,
    }
});

#[cfg(test)]
mod tests {
    use super::*;

    /// The doctest of `ErrorState`.
    #[test]
    fn doctest() {
        let root = ErrorState::new("root", "long msg root", None, None);
        let leaf = ErrorState::new("leaf", "long msg leaf", Some(root), None);
        assert_eq!(leaf.ok(), None);
        assert_eq!(leaf.next(), None);
        assert_eq!(root.prev(), None);
        assert!(root.next() == Some(leaf) && leaf.prev() == Some(root));
        assert!(!leaf.is_ok());
        root.set_ok(false);
        assert_eq!(root.ok(), Some(false));
        leaf.set_ok(true);
        assert!(root.is_ok() && leaf.is_ok());
        assert_eq!(leaf.to_string(), "leaf: True");
    }

    #[test]
    fn false_runs_forward_and_calls_back_with_the_old_value() {
        let a = ErrorState::new("a", "", None, Some(""));
        let b = ErrorState::new("b", "", Some(a), None);
        let c = ErrorState::new("c", "", Some(b), None);
        assert_eq!(a.short_msg(), "a");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        c.set_detected_cb(Arc::new(move |old| log.lock().unwrap().push(old)));
        b.set_ok(false);
        assert_eq!((a.ok(), b.ok(), c.ok()), (None, Some(false), Some(false)));
        assert_eq!(*seen.lock().unwrap(), vec![None]);
        c.set_ok(true);
        assert_eq!(
            (a.ok(), b.ok(), c.ok()),
            (Some(true), Some(true), Some(true))
        );
        assert_eq!(a.iter(), vec![a, b, c]);
    }

    #[test]
    fn module_chain() {
        let chain = &*CHAIN;
        assert_eq!(
            chain.internet.iter(),
            vec![chain.internet, chain.http, chain.token, chain.api]
        );
        assert_eq!(chain.token.short_msg(), "Token");
    }
}

/// `tests/test_errors.py` at `c0d7d233`. Both tests share the module's chain, which is fresh
/// only once per process, so they run in one test, in the file's order.
#[cfg(test)]
mod sdk_tests {
    use super::*;

    #[test]
    fn test_error_states_then_test_errors_iterating() {
        let Chain {
            internet,
            http,
            token,
            api,
        } = *CHAIN;

        // test_error_states: initial checks
        assert!(!internet.is_ok() && !http.is_ok() && !token.is_ok() && !api.is_ok());

        assert_eq!(internet.prev(), None);
        assert_eq!(api.next(), None);

        assert_eq!(internet.next(), Some(http));
        assert_eq!(http.next(), Some(token));
        assert_eq!(token.next(), Some(api));
        assert_eq!(api.prev(), Some(token));
        assert_eq!(token.prev(), Some(http));
        assert_eq!(http.prev(), Some(internet));

        // state propagation: backward
        http.set_ok(true);
        assert_eq!(internet.ok(), Some(true));
        assert_eq!(http.ok(), Some(true));
        assert!(!token.is_ok());
        assert!(!api.is_ok());

        // forward
        api.set_ok(true);
        http.set_ok(false);
        assert_eq!(http.ok(), Some(false));
        assert_eq!(token.ok(), Some(false));
        assert_eq!(api.ok(), Some(false)); // previous state failed so this must fail too
        assert_eq!(internet.ok(), Some(true)); // internet might be OK when HTTP fails

        // test_errors_iterating
        api.set_ok(true);
        for error in internet.iter() {
            assert_eq!(error.ok(), Some(true));
        }

        internet.set_ok(false);
        for error in internet.iter() {
            assert_eq!(error.ok(), Some(false));
        }
    }
}
