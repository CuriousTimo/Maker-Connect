//! `blinker.Signal` 1.9.0 (`blinker/base.py`, MIT, Copyright Jason Kirtland), the part PrusaLink
//! uses: `Signal()`, `connect(receiver, weak=...)`, `disconnect(receiver)`, `send(sender, **kwargs)`
//! and the `receivers` map (`len`, truth). A ported second source, as the reference uses it (owner,
//! 2026-11-16; `docs/archive/plans/mk3-port-plan.md` §5).
//!
//! A receiver is an `Arc` closure, its id the `Arc`'s address (`make_id`, `_utilities.py:45-57`:
//! the identity of a function or bound method). The sender and the keyword arguments are one
//! value of the signal's own type, `A`.
//!
//! **Order:** `send` calls the receivers of a set of ids (`receivers_for`, `:326-362`), an order
//! the source calls "undefined" (`send`, `:224-226`); ours is the order of connecting.
//!
//! **Not covered:** connecting to one sender (`sender=`; PrusaLink connects to `ANY` only) and
//! the sender bookkeeping (`_by_sender` beyond `ANY`, `_weak_senders`, `has_receivers_for`);
//! `receiver_connected` and `receiver_disconnected` (`:41-71`: nobody connects to them),
//! `connect_via`, `connected_to`, `muted`, `send_async`, coroutine receivers, `NamedSignal`,
//! `Namespace`, `_cleanup_bookkeeping`, `_clear_state`; `send`'s `(receiver, result)` list
//! (PrusaLink never reads it, and the receivers return nothing).

use std::sync::{Arc, Mutex, Weak};

/// A receiver: called with the sender and the keyword arguments.
pub type Receiver<A> = dyn Fn(&A) + Send + Sync;

/// `self.receivers[receiver_id]`: the receiver itself, or a `weakref` to it (`make_ref`).
enum Ref<A: ?Sized> {
    Strong(Arc<Receiver<A>>),
    Weak(Weak<Receiver<A>>),
}

/// `make_id(receiver)`
fn make_id<A>(receiver: &Arc<Receiver<A>>) -> usize {
    Arc::as_ptr(receiver) as *const () as usize
}

struct State<A> {
    /// `self.receivers`: a dict, so in the order of connecting.
    receivers: Vec<(usize, Ref<A>)>,
    /// `self._by_sender[ANY_ID]`
    by_sender_any: Vec<usize>,
}

/// `Signal` (`:24`): A notification emitter.
pub struct Signal<A> {
    state: Mutex<State<A>>,
}

impl<A> Default for Signal<A> {
    fn default() -> Self {
        Signal::new()
    }
}

impl<A> Signal<A> {
    /// `__init__(doc=None)` (`:73-89`)
    pub fn new() -> Signal<A> {
        Signal {
            state: Mutex::new(State {
                receivers: vec![],
                by_sender_any: vec![],
            }),
        }
    }

    /// `connect(receiver, sender=ANY, weak=True)` (`:91-139`): Connect `receiver` to be called
    /// when the signal is sent. `weak`: Track the receiver with a weakref. The receiver will be
    /// automatically disconnected when it is garbage collected (here: when its last `Arc` is
    /// dropped).
    pub fn connect(&self, receiver: &Arc<Receiver<A>>, weak: bool) {
        let receiver_id = make_id(receiver);
        let entry = if weak {
            Ref::Weak(Arc::downgrade(receiver))
        } else {
            Ref::Strong(receiver.clone())
        };
        let mut state = self.state.lock().unwrap();
        // a dict: a key set again keeps its place
        match state
            .receivers
            .iter_mut()
            .find(|(id, _)| *id == receiver_id)
        {
            Some(slot) => slot.1 = entry,
            None => state.receivers.push((receiver_id, entry)),
        }
        if !state.by_sender_any.contains(&receiver_id) {
            state.by_sender_any.push(receiver_id);
        }
    }

    /// `send(sender, **kwargs)` (`:204-253`): Call all receivers. Each receiver is called with
    /// `sender` as a positional argument along with any extra keyword arguments. If a receiver
    /// raises an exception, that exception will propagate up.
    pub fn send(&self, args: &A) {
        for receiver in self.receivers_for() {
            receiver(args);
        }
    }

    /// `receivers_for(sender)` (`:326-362`): Yield each receiver to be called. Weakly referenced
    /// receivers that are not live will be disconnected and skipped. A generator: the ids are
    /// taken at the start (`ids = self._by_sender[ANY_ID].copy()`), each receiver is looked up
    /// when its turn comes, without the lock, so a receiver may connect and disconnect.
    fn receivers_for(&self) -> impl Iterator<Item = Arc<Receiver<A>>> + '_ {
        let ids: Vec<usize> = {
            let state = self.state.lock().unwrap();
            if state.receivers.is_empty() {
                vec![]
            } else {
                state.by_sender_any.clone()
            }
        };
        ids.into_iter().filter_map(move |receiver_id| {
            let mut state = self.state.lock().unwrap();
            let receiver = state
                .receivers
                .iter()
                .find(|(id, _)| *id == receiver_id)
                .map(|(_, r)| match r {
                    Ref::Strong(strong) => Some(strong.clone()),
                    Ref::Weak(weak) => weak.upgrade(),
                })?;
            if receiver.is_none() {
                Self::_disconnect(&mut state, receiver_id);
            }
            receiver
        })
    }

    /// `disconnect(receiver, sender=ANY)` (`:364-386`): Disconnect `receiver` from being called
    /// when the signal is sent.
    pub fn disconnect(&self, receiver: &Arc<Receiver<A>>) {
        let receiver_id = make_id(receiver);
        Self::_disconnect(&mut self.state.lock().unwrap(), receiver_id);
    }

    /// `_disconnect(receiver_id, ANY_ID)` (`:388-397`)
    fn _disconnect(state: &mut State<A>, receiver_id: usize) {
        state.by_sender_any.retain(|id| *id != receiver_id);
        state.receivers.retain(|(id, _)| *id != receiver_id);
    }

    /// `len(self.receivers)`. A weakref's callback disconnects a receiver when it is collected
    /// (`_make_cleanup_receiver`, `:399-412`): here those that are gone are disconnected first.
    pub fn receivers_len(&self) -> usize {
        let mut state = self.state.lock().unwrap();
        let dead: Vec<usize> = state
            .receivers
            .iter()
            .filter(|(_, r)| matches!(r, Ref::Weak(weak) if weak.strong_count() == 0))
            .map(|(id, _)| *id)
            .collect();
        for receiver_id in dead {
            Self::_disconnect(&mut state, receiver_id);
        }
        state.receivers.len()
    }

    /// `bool(self.receivers)` (`if s.receivers:`)
    pub fn has_receivers(&self) -> bool {
        self.receivers_len() != 0
    }
}

#[cfg(test)]
mod tests;
