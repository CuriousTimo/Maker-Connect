//! Port of the parts of OctoPrint 1.11.8 `src/octoprint/plugin/` the comm layer, `printer/standard.py`
//! and the prompt plugin ask for: `octoprint.plugin.plugin_manager()` (`plugin/__init__.py:42`),
//! `PluginManager.get_hooks(hook)` (`plugin/core.py:2149`), `register_message_receiver` (:2281),
//! `unregister_message_receiver` (:2292) and `send_plugin_message` (:2303).
//!
//! There is no plugin loading. The one plugin the port has is `action_command_prompt.rs`
//! (`docs/archive/plans/agent-engine.md`); it registers the hooks of its `__plugin_hooks__`
//! (`plugins/action_command_prompt/__init__.py:297-302`) with [`PluginManager::register_hook`]. Until
//! one is registered `get_hooks` takes its first branch (`if hook not in self.plugin_hooks:
//! return {}`, core.py:2160-2161). The result of the other branch is an `OrderedDict` of plugin
//! identifier -> handler (core.py:2163-2170), so the type is [`Hooks`], an ordered map of callables.
//!
//! OURS: the handler signature. Python hooks are callables with a signature per hook
//! (`octoprint.comm.protocol.gcode.queuing(comm_instance, phase, cmd, cmd_type, gcode, ...)`);
//! a handler is `Fn(&[Value]) -> Value`, called without the comm instance (comm.rs, difference 20,
//! 31). OURS: `register_hook` stands in for what plugin loading does (`_import_hooks`), and keeps
//! the handlers in registration order (Python sorts them by `(order, ...)`, core.py:2318; with one
//! plugin that is the same). Not ported: `time_this` timing wrapper (core.py:2165), plugin loading.

use super::py_collections::PyDict;
use crate::ports::python::py_threading::LockExt;
use serde_json::Value;
use std::sync::{Arc, Mutex, OnceLock};

/// A hook handler (ours, see the module doc).
pub type HookFn = Arc<dyn Fn(&[Value]) -> Value + Send + Sync>;

/// The result of `get_hooks`: plugin identifier -> handler, in order.
pub type Hooks = PyDict<HookFn>;

/// A plugin message receiver: `client(plugin, data, permissions=None)` (core.py:2281); the
/// `permissions` are for the web client and are not passed.
pub type MessageReceiver = Arc<dyn Fn(&str, &Value) + Send + Sync>;

/// `PluginManager` (core.py), only `plugin_hooks` / `get_hooks` and the message receivers.
pub struct PluginManager {
    /// `self.plugin_hooks` (hook name -> list of (plugin identifier, handler))
    plugin_hooks: Mutex<Vec<(String, Vec<(String, HookFn)>)>>,
    /// `self.registered_clients` (core.py:951)
    registered_clients: Mutex<Vec<MessageReceiver>>,
}

static INSTANCE: OnceLock<PluginManager> = OnceLock::new();

/// `plugin/__init__.py:42 plugin_manager()`, without the `init=True` arguments (there is nothing
/// to initialise).
pub fn plugin_manager() -> &'static PluginManager {
    INSTANCE.get_or_init(PluginManager::new)
}

impl PluginManager {
    /// OURS: an empty manager (no plugin loaded)
    pub fn new() -> PluginManager {
        PluginManager {
            plugin_hooks: Mutex::new(Vec::new()),
            registered_clients: Mutex::new(Vec::new()),
        }
    }

    /// OURS: what plugin loading does for one entry of a plugin's `__plugin_hooks__`
    pub fn register_hook(&self, hook: &str, identifier: &str, handler: HookFn) {
        let mut hooks = self.plugin_hooks.plock();
        match hooks.iter_mut().find(|(k, _)| k == hook) {
            Some((_, handlers)) => handlers.push((identifier.to_string(), handler)),
            None => hooks.push((hook.to_string(), vec![(identifier.to_string(), handler)])),
        }
    }

    /// `core.py:2149 get_hooks`
    pub fn get_hooks(&self, hook: &str) -> Hooks {
        let hooks = self.plugin_hooks.plock();
        let Some((_, handlers)) = hooks.iter().find(|(k, _)| k == hook) else {
            return Hooks::new();
        };
        let mut result = Hooks::new();
        for (identifier, handler) in handlers {
            // `result[h[0]] = time_this(...)(h[1])` (the timing wrapper is not ported)
            result.insert(identifier, handler.clone());
        }
        result
    }

    /// `core.py:2281 register_message_receiver(client)`
    pub fn register_message_receiver(&self, client: MessageReceiver) {
        self.registered_clients.plock().push(client);
    }

    /// `core.py:2292 unregister_message_receiver(client)`
    pub fn unregister_message_receiver(&self, client: &MessageReceiver) {
        let mut clients = self.registered_clients.plock();
        if let Some(i) = clients.iter().position(|c| Arc::ptr_eq(c, client)) {
            clients.remove(i);
        }
    }

    /// `core.py:2303 send_plugin_message(plugin, data, permissions=None)`
    pub fn send_plugin_message(&self, plugin: &str, data: &Value) {
        let clients: Vec<MessageReceiver> = self.registered_clients.plock().clone();
        for client in clients {
            // except Exception: self.logger.exception("Exception while sending plugin data to client")
            let _ = super::comm::py_try(|| client(plugin, data));
        }
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}
