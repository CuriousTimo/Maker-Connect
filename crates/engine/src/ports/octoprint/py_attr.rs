//! OURS (not a port of any source file): `Attr<T>`, the stand-in for a Python object attribute.
//!
//! In CPython a read or a write of an attribute (`self._x`, `self._x = v`) is atomic on its own
//! (one bytecode, under the GIL), and nothing more: `self._x += 1` is a read and a write, and a
//! compound check-then-act is not atomic. `Attr<T>` gives exactly that and no more:
//! - [`Attr::get`] is the read (a clone, like reading a reference; for `Arc<..>` it is the
//!   reference), [`Attr::set`] is the write;
//! - [`Attr::with_mut`] is ONE call of a method of the object the attribute holds
//!   (`self._tempOffsets.update(..)`, `self._lastLines.append(..)`, `self._sdFiles[k] = v`): a
//!   single built-in `dict`/`set`/`deque` operation is atomic under the GIL, so it runs under the
//!   cell's mutex. Never hold it over more than that one operation: Python would not either.
//!
//! The mutex is not one of Python's locks and is never held across code of the caller other than
//! that one operation. A poisoned mutex is used as it is (Python has no poisoning).

use std::sync::{Mutex, MutexGuard};

pub struct Attr<T>(Mutex<T>);

impl<T> Attr<T> {
    pub fn new(value: T) -> Self {
        Attr(Mutex::new(value))
    }

    fn lock(&self) -> MutexGuard<'_, T> {
        crate::ports::python::py_threading::lock(&self.0)
    }

    /// `x = self._attr`
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.lock().clone()
    }

    /// `self._attr = value`
    pub fn set(&self, value: T) {
        *self.lock() = value;
    }

    /// One method call on the held object: `self._attr.method(...)`.
    pub fn with_mut<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        f(&mut self.lock())
    }

    /// One read-only operation on the held object (`len(self._attr)`, `x in self._attr`,
    /// `self._attr.get(k)`).
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.lock())
    }
}
