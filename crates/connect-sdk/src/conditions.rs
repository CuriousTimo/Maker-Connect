//! A component to handle error conditions (`conditions.py`).
//!
//! Conditions are tree nodes, if a parent node becomes NOK, all children become NOK too. If all
//! children of a node become OK, the parent node becomes OK.
//!
//! Use the tracker to get the most important error according to the tracked condition
//! priorities. This should make it easy to show the same error independent of whether it's the
//! LCD or the web.
//!
//! A [`Condition`] is a handle into one process-wide store, as the Python objects are module
//! globals. `cond_lock` is a re-entrant lock held for each change and while its handlers run, as
//! the Python `RLock`. Python keeps handlers in sets; here they run in the order they were
//! added, and [`ConditionTracker::get_worst`] picks, among equal priorities, the last one to
//! break.

use std::fmt;
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard};
use std::thread::{self, ThreadId};

/// Describes the possible condition states
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CondState {
    Unknown,
    Ok,
    Nok,
}

impl CondState {
    /// `.name`
    pub fn name(self) -> &'static str {
        match self {
            CondState::Unknown => "UNKNOWN",
            CondState::Ok => "OK",
            CondState::Nok => "NOK",
        }
    }
}

/// The `ValueError`s of `conditions.py`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionError {
    /// "Can't reset a condition to unknown"
    ResetToUnknown,
    /// "Can't replace a parent with another"
    ReplaceParent,
    /// "Re-planting of initialized trees is not supported."
    Replant,
    /// "Can't add an already added handler"
    HandlerAdded,
    /// The `KeyError` of removing a handler that was not added.
    HandlerNotAdded,
}

impl fmt::Display for ConditionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ConditionError::ResetToUnknown => "Can't reset a condition to unknown",
            ConditionError::ReplaceParent => "Can't replace a parent with another",
            ConditionError::Replant => "Re-planting of initialized trees is not supported.",
            ConditionError::HandlerAdded => "Can't add an already added handler",
            ConditionError::HandlerNotAdded => "handler not added",
        })
    }
}

impl std::error::Error for ConditionError {}

/// A broke or fixed handler: called with the condition and its old state. A handler is the
/// same handler when it is the same `Arc`, as Python compares the function object.
pub type Handler = Arc<dyn Fn(Condition, CondState) + Send + Sync>;

fn same_handler(a: &Handler, b: &Handler) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b))
}

#[derive(Clone)]
enum Callback {
    /// `child._parent_broke`, on the parent's broke handlers
    ParentBroke(usize),
    /// `parent._child_fixed`, on the child's fixed handlers
    ChildFixed(usize),
    External(Handler),
}

struct Node {
    name: String,
    long_msg: String,
    short_msg: String,
    priority: i64,
    broke_cb: Vec<Callback>,
    fixed_cb: Vec<Callback>,
    parent: Option<usize>,
    children: Vec<usize>,
    state: CondState,
}

struct Store {
    nodes: Vec<Node>,
}

static STORE: Mutex<Store> = Mutex::new(Store { nodes: Vec::new() });

fn store() -> MutexGuard<'static, Store> {
    STORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `multiprocessing.RLock()`: re-entrant for the thread that holds it.
struct RLock {
    owner: Mutex<(Option<ThreadId>, usize)>,
    released: Condvar,
}

struct RLockGuard<'a>(&'a RLock);

impl RLock {
    const fn new() -> RLock {
        RLock {
            owner: Mutex::new((None, 0)),
            released: Condvar::new(),
        }
    }

    fn lock(&self) -> RLockGuard<'_> {
        let me = thread::current().id();
        let mut owner = self.owner.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            match owner.0 {
                None => {
                    *owner = (Some(me), 1);
                    break;
                }
                Some(id) if id == me => {
                    owner.1 += 1;
                    break;
                }
                Some(_) => owner = self.released.wait(owner).unwrap_or_else(|p| p.into_inner()),
            }
        }
        RLockGuard(self)
    }
}

impl Drop for RLockGuard<'_> {
    fn drop(&mut self) {
        let mut owner = self.0.owner.lock().unwrap_or_else(|p| p.into_inner());
        owner.1 -= 1;
        if owner.1 == 0 {
            owner.0 = None;
            self.0.released.notify_one();
        }
    }
}

static COND_LOCK: RLock = RLock::new();

/// A more detailed condition for state tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Condition(usize);

impl Condition {
    pub fn new(
        name: &str,
        long_msg: &str,
        parent: Option<Condition>,
        short_msg: Option<&str>,
        priority: i64,
    ) -> Result<Condition, ConditionError> {
        let condition = {
            let mut store = store();
            let index = store.nodes.len();
            let short_msg = short_msg.filter(|s| !s.is_empty()).unwrap_or(name);
            store.nodes.push(Node {
                name: name.to_owned(),
                long_msg: long_msg.to_owned(),
                short_msg: short_msg.to_owned(),
                priority,
                broke_cb: Vec::new(),
                fixed_cb: Vec::new(),
                parent: None,
                children: Vec::new(),
                state: CondState::Unknown,
            });
            Condition(index)
        };
        if let Some(parent) = parent {
            condition.set_parent(parent)?;
        }
        Ok(condition)
    }

    /// Add a parent condition - meant for joining condition trees
    pub fn set_parent(self, parent: Condition) -> Result<(), ConditionError> {
        let _lock = COND_LOCK.lock();
        let mut store = store();
        let current = store.nodes[self.0].parent;
        if current == Some(parent.0) {
            return Ok(());
        }
        if current.is_some() {
            return Err(ConditionError::ReplaceParent);
        }
        let unknown = |c: Condition| store.nodes[c.0].state == CondState::Unknown;
        if !(unknown(parent) && unknown(self)) {
            return Err(ConditionError::Replant);
        }
        store.nodes[self.0].parent = Some(parent.0);
        store.nodes[parent.0]
            .broke_cb
            .push(Callback::ParentBroke(self.0));
        store.nodes[parent.0].children.push(self.0);
        store.nodes[self.0]
            .fixed_cb
            .push(Callback::ChildFixed(parent.0));
        Ok(())
    }

    /// Automatically become broken, if our parent breaks
    fn parent_broke(self) {
        let _ = self.set_state(CondState::Nok);
    }

    /// Automatically become OK if all of our children are OK
    fn child_fixed(self) {
        if self.children().into_iter().all(Condition::is_ok) {
            let _ = self.set_state(CondState::Ok);
        }
    }

    fn handlers(store: &mut Store, condition: Condition, broke: bool) -> &mut Vec<Callback> {
        let node = &mut store.nodes[condition.0];
        if broke {
            &mut node.broke_cb
        } else {
            &mut node.fixed_cb
        }
    }

    fn add_handler(self, handler: Handler, broke: bool) -> Result<(), ConditionError> {
        let _lock = COND_LOCK.lock();
        let mut store = store();
        let list = Self::handlers(&mut store, self, broke);
        if list
            .iter()
            .any(|cb| matches!(cb, Callback::External(h) if same_handler(h, &handler)))
        {
            return Err(ConditionError::HandlerAdded);
        }
        list.push(Callback::External(handler));
        Ok(())
    }

    fn remove_handler(self, handler: &Handler, broke: bool) -> Result<(), ConditionError> {
        let _lock = COND_LOCK.lock();
        let mut store = store();
        let list = Self::handlers(&mut store, self, broke);
        let position = list
            .iter()
            .position(|cb| matches!(cb, Callback::External(h) if same_handler(h, handler)))
            .ok_or(ConditionError::HandlerNotAdded)?;
        list.remove(position);
        Ok(())
    }

    /// Adds a handler for a condition breaking
    pub fn add_broke_handler(self, handler: Handler) -> Result<(), ConditionError> {
        self.add_handler(handler, true)
    }

    /// Adds a handler for a condition fixing itself
    pub fn add_fixed_handler(self, handler: Handler) -> Result<(), ConditionError> {
        self.add_handler(handler, false)
    }

    /// Removes a handler for a condition breaking
    pub fn remove_broke_handler(self, handler: &Handler) -> Result<(), ConditionError> {
        self.remove_handler(handler, true)
    }

    /// Removes a handler for a condition fixing itself
    pub fn remove_fixed_handler(self, handler: &Handler) -> Result<(), ConditionError> {
        self.remove_handler(handler, false)
    }

    pub fn name(self) -> String {
        store().nodes[self.0].name.clone()
    }

    pub fn long_msg(self) -> String {
        store().nodes[self.0].long_msg.clone()
    }

    pub fn short_msg(self) -> String {
        store().nodes[self.0].short_msg.clone()
    }

    /// Returns this node's parent
    pub fn parent(self) -> Option<Condition> {
        store().nodes[self.0].parent.map(Condition)
    }

    pub fn children(self) -> Vec<Condition> {
        store().nodes[self.0]
            .children
            .iter()
            .copied()
            .map(Condition)
            .collect()
    }

    /// Returns the CondState value
    pub fn state(self) -> CondState {
        store().nodes[self.0].state
    }

    /// Sets the condition using the CondState
    pub fn set_state(self, state: CondState) -> Result<(), ConditionError> {
        if state == CondState::Unknown {
            return Err(ConditionError::ResetToUnknown);
        }
        // Make sure only one thread is changing our states
        let _lock = COND_LOCK.lock();
        let (old_state, handlers) = {
            let mut store = store();
            let node = &mut store.nodes[self.0];
            if node.state == state {
                // skip handlers
                return Ok(());
            }
            let old_state = node.state;
            node.state = state;
            let handlers = if state == CondState::Ok {
                &node.fixed_cb
            } else {
                &node.broke_cb
            };
            (old_state, handlers.clone())
        };
        for handler in handlers {
            match handler {
                Callback::ParentBroke(child) => Condition(child).parent_broke(),
                Callback::ChildFixed(parent) => Condition(parent).child_fixed(),
                Callback::External(handler) => handler(self, old_state),
            }
        }
        Ok(())
    }

    pub fn priority(self) -> i64 {
        store().nodes[self.0].priority
    }

    /// Returns True if all the successors are OK (this one included, as in Python)
    pub fn successors_ok(self) -> bool {
        self.iter().into_iter().all(Condition::is_ok)
    }

    /// Returns True only if we know we're OK (`__bool__`)
    pub fn is_ok(self) -> bool {
        self.state() == CondState::Ok
    }

    /// `__iter__`: every child's subtree, then this one.
    pub fn iter(self) -> Vec<Condition> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(self, out: &mut Vec<Condition>) {
        for child in self.children() {
            child.collect(out);
        }
        out.push(self);
    }
}

impl fmt::Display for Condition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.name(), self.state().name())
    }
}

#[derive(Default)]
struct Tracked {
    nok_conditions: Vec<Condition>,
    tracked_conditions: Vec<(Condition, Handler, Handler)>,
}

/// Tracks the NOK conditions and their priorities
///
/// The time complexity is O(n), but the number of items is expected to be low
#[derive(Clone, Default)]
pub struct ConditionTracker {
    inner: Arc<Mutex<Tracked>>,
}

impl ConditionTracker {
    pub fn new() -> ConditionTracker {
        ConditionTracker::default()
    }

    fn tracked(&self) -> MutexGuard<'_, Tracked> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Add a tree of conditions to be tracked
    pub fn add_tracked_condition_tree(&self, root_condition: Condition) {
        let _lock = COND_LOCK.lock();
        for condition in root_condition.iter() {
            self.add_tracked_condition(condition);
        }
    }

    /// Add a conditions to be tracked
    pub fn add_tracked_condition(&self, condition: Condition) {
        let _lock = COND_LOCK.lock();
        if self.is_tracked(condition) {
            return;
        }
        let tracker = Arc::downgrade(&self.inner);
        let broke: Handler = Arc::new(move |c, _| {
            if let Some(inner) = tracker.upgrade() {
                ConditionTracker { inner }.tracked_broke(c);
            }
        });
        let tracker = Arc::downgrade(&self.inner);
        let fixed: Handler = Arc::new(move |c, _| {
            if let Some(inner) = tracker.upgrade() {
                ConditionTracker { inner }.tracked_fixed(c);
            }
        });
        // new Arcs, so never already added
        let _ = condition.add_broke_handler(broke.clone());
        let _ = condition.add_fixed_handler(fixed.clone());
        self.tracked()
            .tracked_conditions
            .push((condition, broke, fixed));
        if condition.state() == CondState::Nok {
            self.tracked_broke(condition);
        }
    }

    /// Remove a tree of conditions, so they're not tracked anymore
    pub fn remove_tracked_condition_tree(&self, condition_root: Condition) {
        let _lock = COND_LOCK.lock();
        for condition in condition_root.iter() {
            self.remove_tracked_condition(condition);
        }
    }

    /// Remove a condition, so it's not tracked anymore
    pub fn remove_tracked_condition(&self, condition: Condition) {
        let _lock = COND_LOCK.lock();
        let entry = {
            let mut tracked = self.tracked();
            let position = tracked
                .tracked_conditions
                .iter()
                .position(|(c, _, _)| *c == condition);
            position.map(|i| tracked.tracked_conditions.remove(i))
        };
        if let Some((_, broke, fixed)) = entry {
            let _ = condition.remove_broke_handler(&broke);
            let _ = condition.remove_fixed_handler(&fixed);
        }
        self.tracked_fixed(condition);
    }

    /// Handle a tracked condition breaking by adding it to the broken ones
    fn tracked_broke(&self, condition: Condition) {
        let mut tracked = self.tracked();
        if !tracked.nok_conditions.contains(&condition) {
            tracked.nok_conditions.push(condition);
        }
    }

    /// Handle a tracked condition fixing itself - remove it to the broken ones
    fn tracked_fixed(&self, condition: Condition) {
        self.tracked().nok_conditions.retain(|c| *c != condition);
    }

    /// Gets every tracked nok condition
    pub fn nok_conditions(&self) -> Vec<Condition> {
        self.tracked().nok_conditions.clone()
    }

    /// Gets the broken condition with the highest priority. Returns None if there's no broken
    /// ones
    pub fn get_worst(&self) -> Option<Condition> {
        let _lock = COND_LOCK.lock();
        let mut worst_error = None;
        let mut worst_priority = i64::MIN;
        for condition in self.nok_conditions() {
            let priority = condition.priority();
            if priority >= worst_priority {
                worst_error = Some(condition);
                worst_priority = priority;
            }
        }
        worst_error
    }

    /// Is the specified condition being tracked
    pub fn is_tracked(&self, condition: Condition) -> bool {
        self.tracked()
            .tracked_conditions
            .iter()
            .any(|(c, _, _)| *c == condition)
    }
}

/// The module's condition chain and its tracker, created together on first use.
pub struct Chain {
    pub internet: Condition,
    pub http: Condition,
    pub token: Condition,
    pub api: Condition,
    pub tracker: ConditionTracker,
}

/// The same condition chain as before: error chain representing a kind of semaphore signaling
/// the status of the connection to Connect, tracked by `COND_TRACKER`.
pub static CHAIN: LazyLock<Chain> = LazyLock::new(|| {
    let new = |name, msg, parent, priority| {
        Condition::new(name, msg, parent, None, priority).expect("a new tree")
    };
    let internet = new(
        "Internet",
        "DNS does not work, or there are other problems in communication to other hosts in the \
         Internet.",
        None,
        130,
    );
    let http = new(
        "HTTP",
        "HTTP communication to Connect fails, we're getting 5XX statuses",
        Some(internet),
        120,
    );
    // Signal if we have a token or not
    let token = new(
        "Token",
        "Printer has no valid token, it needs to be registered with Connect.",
        Some(http),
        110,
    );
    let api = new(
        "API",
        "Encountered 4XX problems while communicating to Connect",
        Some(token),
        100,
    );
    let tracker = ConditionTracker::new();
    tracker.add_tracked_condition_tree(internet);
    Chain {
        internet,
        http,
        token,
        api,
        tracker,
    }
});

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> (Condition, Condition, Condition, Condition) {
        let root = Condition::new("Root", "root", None, None, 30).unwrap();
        let mid = Condition::new("Mid", "mid", Some(root), None, 20).unwrap();
        let leaf_a = Condition::new("A", "a", Some(mid), Some("short a"), 10).unwrap();
        let leaf_b = Condition::new("B", "b", Some(mid), None, 10).unwrap();
        (root, mid, leaf_a, leaf_b)
    }

    #[test]
    fn a_broken_parent_breaks_every_child() {
        let (root, mid, a, b) = tree();
        root.set_state(CondState::Nok).unwrap();
        for c in [root, mid, a, b] {
            assert_eq!(c.state(), CondState::Nok, "{c}");
        }
    }

    #[test]
    fn a_parent_is_fixed_once_all_children_are() {
        let (root, mid, a, b) = tree();
        root.set_state(CondState::Nok).unwrap();
        a.set_state(CondState::Ok).unwrap();
        assert_eq!(
            (mid.state(), root.state()),
            (CondState::Nok, CondState::Nok)
        );
        b.set_state(CondState::Ok).unwrap();
        assert_eq!((mid.state(), root.state()), (CondState::Ok, CondState::Ok));
        // fixing a parent does not fix its children
        a.set_state(CondState::Nok).unwrap();
        mid.set_state(CondState::Ok).unwrap();
        assert_eq!(a.state(), CondState::Nok);
    }

    #[test]
    fn errors_and_names() {
        let (root, mid, a, b) = tree();
        assert_eq!(
            a.set_state(CondState::Unknown),
            Err(ConditionError::ResetToUnknown)
        );
        let other = Condition::new("Other", "", None, None, 0).unwrap();
        assert_eq!(a.set_parent(other), Err(ConditionError::ReplaceParent));
        assert_eq!(a.set_parent(mid), Ok(()));
        root.set_state(CondState::Ok).unwrap();
        let orphan = Condition::new("Orphan", "", None, None, 0).unwrap();
        assert_eq!(orphan.set_parent(root), Err(ConditionError::Replant));
        assert_eq!(a.short_msg(), "short a");
        assert_eq!(b.short_msg(), "B");
        assert_eq!(a.to_string(), "A: UNKNOWN");
        assert_eq!(root.iter(), vec![a, b, mid, root]);
        assert!(!root.successors_ok());
    }

    #[test]
    fn handlers_get_the_old_state_and_can_be_removed() {
        let (root, _, a, _) = tree();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let handler: Handler = Arc::new(move |c, old| log.lock().unwrap().push((c, old)));
        a.add_broke_handler(handler.clone()).unwrap();
        root.set_state(CondState::Nok).unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![(a, CondState::Unknown)]);
        assert_eq!(a.remove_broke_handler(&handler), Ok(()));
        assert_eq!(
            a.remove_broke_handler(&handler),
            Err(ConditionError::HandlerNotAdded)
        );
    }

    #[test]
    fn a_handler_may_change_conditions() {
        let (root, _, a, _) = tree();
        let other = Condition::new("Other", "", None, None, 0).unwrap();
        a.add_broke_handler(Arc::new(move |_, _| {
            other.set_state(CondState::Nok).unwrap()
        }))
        .unwrap();
        root.set_state(CondState::Nok).unwrap();
        assert_eq!(other.state(), CondState::Nok);
    }

    #[test]
    fn tracker_reports_the_worst() {
        let (root, mid, a, b) = tree();
        let tracker = ConditionTracker::new();
        a.set_state(CondState::Nok).unwrap();
        tracker.add_tracked_condition_tree(root);
        assert!(tracker.is_tracked(b));
        assert_eq!(tracker.get_worst(), Some(a));
        root.set_state(CondState::Nok).unwrap();
        assert_eq!(tracker.get_worst(), Some(root));
        assert_eq!(tracker.nok_conditions().len(), 4);
        for c in [a, b] {
            c.set_state(CondState::Ok).unwrap();
        }
        assert_eq!(tracker.get_worst(), None);
        mid.set_state(CondState::Nok).unwrap();
        tracker.remove_tracked_condition(mid);
        // a and b broke with mid (equal priority): the last to break wins
        assert_eq!(tracker.nok_conditions(), vec![a, b]);
        assert_eq!(tracker.get_worst(), Some(b));
        assert!(!tracker.nok_conditions().contains(&mid));
        assert!(!tracker.is_tracked(mid));
    }

    #[test]
    fn module_chain() {
        let chain = &*CHAIN;
        assert_eq!(chain.api.parent(), Some(chain.token));
        assert!(chain.tracker.is_tracked(chain.api));
        assert_eq!(chain.internet.priority(), 130);
    }
}

/// `tests/test_conditions.py` at `c0d7d233`, test by test.
#[cfg(test)]
mod sdk_tests {
    use super::*;
    use std::collections::HashSet;

    type Calls = Arc<Mutex<Vec<(Condition, CondState)>>>;

    /// `unittest.mock.Mock()` as a handler.
    fn mock() -> (Handler, Calls) {
        let calls: Calls = Arc::default();
        let log = calls.clone();
        let handler: Handler = Arc::new(move |c, old| log.lock().unwrap().push((c, old)));
        (handler, calls)
    }

    fn cond(name: &str, parent: Option<Condition>, priority: i64) -> Condition {
        Condition::new(name, "Lorem ipsum", parent, None, priority).unwrap()
    }

    fn set(c: Condition, state: CondState) {
        c.set_state(state).unwrap();
    }

    use CondState::{Nok, Ok as OK};

    #[test]
    fn test_sanity() {
        let root = cond("Root", None, 0);
        let child = cond("Child", Some(root), 1);
        assert_eq!(child.parent(), Some(root));
        assert!(root.children().contains(&child));
    }

    #[test]
    fn test_callbacks() {
        let root = cond("Root", None, 0);
        let (broke_callback, broke_calls) = mock();
        let (fixed_callback, fixed_calls) = mock();
        root.add_broke_handler(broke_callback).unwrap();
        root.add_fixed_handler(fixed_callback).unwrap();
        set(root, Nok);
        assert_eq!(
            *broke_calls.lock().unwrap(),
            vec![(root, CondState::Unknown)]
        );
        set(root, OK);
        assert_eq!(*fixed_calls.lock().unwrap(), vec![(root, Nok)]);
    }

    #[test]
    fn test_removing_callbacks() {
        let root = cond("Root", None, 0);
        let (broke_callback, broke_calls) = mock();
        let (fixed_callback, fixed_calls) = mock();
        root.add_broke_handler(broke_callback.clone()).unwrap();
        root.add_fixed_handler(fixed_callback.clone()).unwrap();
        assert_eq!(
            root.add_broke_handler(broke_callback.clone()),
            Err(ConditionError::HandlerAdded)
        );
        assert_eq!(
            root.add_fixed_handler(fixed_callback.clone()),
            Err(ConditionError::HandlerAdded)
        );

        root.remove_broke_handler(&broke_callback).unwrap();
        root.remove_fixed_handler(&fixed_callback).unwrap();
        set(root, Nok);
        assert!(broke_calls.lock().unwrap().is_empty());
        set(root, OK);
        assert!(fixed_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn test_propagation() {
        let grandma = cond("Grandma", None, 0);
        let mom = cond("Mom", Some(grandma), 0);
        let aunt = cond("Aunt", Some(grandma), 0);
        let me = cond("Me", Some(mom), 0);

        set(grandma, OK);
        set(mom, Nok);
        set(aunt, Nok);
        assert_eq!(
            [grandma, mom, aunt, me].map(Condition::state),
            [OK, Nok, Nok, Nok]
        );

        set(grandma, Nok);
        set(mom, OK);
        assert_eq!(
            [grandma, mom, aunt, me].map(Condition::state),
            [Nok, OK, Nok, Nok]
        );

        set(aunt, OK);
        assert_eq!(
            [grandma, mom, aunt, me].map(Condition::state),
            [OK, OK, OK, Nok]
        );
    }

    #[test]
    fn test_tracker_item_management() {
        let tracker = ConditionTracker::new();
        let root = cond("Root", None, 0);
        let a = cond("Node A", Some(root), 0);
        let b = cond("Node B", Some(root), 0);
        let a_l = cond("Node A->L", Some(a), 0);
        let a_m = cond("Node A->M", Some(a), 0);
        let b_n = cond("Node B->N", Some(b), 0);
        let b_o = cond("Node B->O", Some(b), 0);
        for node in [root, a, b, a_l, a_m, b_n, b_o] {
            set(node, OK);
        }

        tracker.add_tracked_condition_tree(a);
        let tracked: HashSet<Condition> = [root, a, b, a_l, a_m, b_n, b_o]
            .into_iter()
            .filter(|c| tracker.is_tracked(*c))
            .collect();
        assert_eq!(tracked, HashSet::from([a, a_l, a_m]));

        set(b_o, Nok);
        tracker.add_tracked_condition_tree(b);
        assert_eq!(tracker.get_worst(), Some(b_o));

        tracker.remove_tracked_condition(b_o);
        assert_eq!(tracker.get_worst(), None);
        assert!(!tracker.is_tracked(b_o));

        // Test that we don't track removed condi
        tracker.remove_tracked_condition_tree(a);
        set(a_l, Nok);
        assert!(![a, a_l, a_m].into_iter().any(|c| tracker.is_tracked(c)));
        assert_eq!(tracker.get_worst(), None);
    }

    #[test]
    fn test_priority() {
        let tracker = ConditionTracker::new();
        let root = cond("Root", None, 0);
        let child = cond("Child", Some(root), 1);
        tracker.add_tracked_condition_tree(root);
        set(root, Nok);
        set(child, Nok);
        assert_eq!(tracker.get_worst(), Some(child));
    }

    #[test]
    fn test_replanting() {
        let root = cond("Root", None, 0);
        let child = cond("Child", Some(root), 0);
        let (broke_handler, calls) = mock();
        child.add_broke_handler(broke_handler).unwrap();

        let new_root = cond("New root", None, 0);
        root.set_parent(new_root).unwrap();
        set(new_root, Nok);
        assert_eq!(*calls.lock().unwrap(), vec![(child, CondState::Unknown)]);
    }

    #[test]
    fn test_forbidden_replanting() {
        let root = cond("Root", None, 0);
        set(root, OK);
        assert_eq!(
            Condition::new("Child", "Lorem ipsum", Some(root), None, 0),
            Err(ConditionError::Replant)
        );
    }

    #[test]
    fn test_set_parent() {
        let root = cond("Root", None, 0);
        let fake_rooot = cond("FakeRoot", None, 0);
        let child = cond("Child", None, 0);

        child.set_parent(root).unwrap();
        // Setting the same parent is ok
        child.set_parent(root).unwrap();
        // Changing the parent is not ok
        assert_eq!(
            child.set_parent(fake_rooot),
            Err(ConditionError::ReplaceParent)
        );
    }

    #[test]
    fn test_bool() {
        let c = cond("Condition", None, 0);
        assert!(!c.is_ok());
        set(c, OK);
        assert!(c.is_ok());
        set(c, Nok);
        assert!(!c.is_ok());
    }

    #[test]
    fn test_descendants() {
        let root = cond("Root", None, 0);
        let a = cond("Node A", Some(root), 0);
        let b = cond("Node B", Some(root), 0);
        let a_l = cond("Node A->L", Some(a), 0);
        let a_m = cond("Node A->M", Some(a), 0);
        let b_n = cond("Node B->N", Some(b), 0);
        let b_o = cond("Node B->O", Some(b), 0);
        for node in [root, a, b, a_l, a_m, b_n] {
            set(node, OK);
        }
        set(b_o, Nok);

        assert!(!root.successors_ok());
        set(b_o, OK);
        assert!(root.successors_ok());
    }

    #[test]
    fn test_init_to_ok() {
        // This scenario was broken, test it does not raise errors
        let tracker = ConditionTracker::new();
        let node = cond("Node", None, 0);
        tracker.add_tracked_condition(node);
        set(node, OK);
    }

    #[test]
    fn test_double_add() {
        // This scenario was broken, test it does not raise errors
        let tracker = ConditionTracker::new();
        let node = cond("Node", None, 0);
        tracker.add_tracked_condition(node);
        tracker.add_tracked_condition(node);
    }

    #[test]
    fn test_double_remove() {
        // This scenario was broken, test it does not raise errors
        let tracker = ConditionTracker::new();
        let node = cond("Node", None, 0);
        tracker.remove_tracked_condition(node);
        tracker.remove_tracked_condition(node);
    }

    #[test]
    fn test_getting_all() {
        let tracker = ConditionTracker::new();
        let root = cond("Root", None, 0);
        let child = cond("Child", Some(root), 0);
        let foo = cond("Foo", None, 0);
        tracker.add_tracked_condition_tree(root);
        tracker.add_tracked_condition(foo);
        set(root, Nok);
        let nok: HashSet<Condition> = tracker.nok_conditions().into_iter().collect();
        assert_eq!(nok, HashSet::from([root, child]));
    }
}
