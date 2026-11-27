//! PrusaLink's `printer_adapter/structures/mc_singleton.py` (`reference/Prusa-Link`, `171148ce`):
//! Contains implementation of the MCSingleton class (#242, section A).
//!
//! `MCSingleton` is a metaclass: one instance per class, per process, `get_instance()` returns it.
//! Here one per printer module, as the agent hosts several (#218; as `Settings.instance`,
//! `config.rs`): a printer module holds an [`MCSingleton`] for each such class (`Job`, `Model`,
//! `SerialQueue`, ...), and makes each instance through it, so a second one fails as in the source.
//!
//! **Not covered:** the metaclass itself (`__init__` setting `cls.__instance`, `:8-11`): Rust has
//! none, the cell is what it sets up.

use std::sync::{Arc, OnceLock};

use maker_connect_gcode_metadata::py::PyErr;

/// `MCSingleton` (`:4-20`): Classes that use this metaclass are singletons
pub struct MCSingleton<T> {
    /// `cls.__instance` (`:9`)
    instance: OnceLock<Arc<T>>,
}

impl<T> Default for MCSingleton<T> {
    fn default() -> Self {
        MCSingleton {
            instance: OnceLock::new(),
        }
    }
}

impl<T> MCSingleton<T> {
    /// `cls.get_instance = lambda: cls.__instance` (`:10`)
    pub fn get_instance(&self) -> Option<Arc<T>> {
        self.instance.get().cloned()
    }

    /// `__call__(cls, *args, **kwargs)` (`:13-20`): `make` is `cls.__new__` and `__init__`.
    pub fn call(&self, make: impl FnOnce() -> T) -> Result<Arc<T>, PyErr> {
        if self.instance.get().is_some() {
            return Err(PyErr::Other(
                "RuntimeError",
                "There can be only one singleton in existence".to_owned(),
            ));
        }

        let instance = Arc::new(make());
        self.instance.set(instance.clone()).map_err(|_| {
            PyErr::Other(
                "RuntimeError",
                "There can be only one singleton in existence".to_owned(),
            )
        })?;
        Ok(instance)
    }
}

#[cfg(test)]
mod tests;
