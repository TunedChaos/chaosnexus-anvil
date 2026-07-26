// chaosnexus-anvil/src/scripting/plugin_context.rs
//
// Trusted caller identity (`CURRENT_PLUGIN`) and capability lookup for native gates.

use crate::scripting::capabilities::{Capability, CapabilitySet};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

thread_local! {
    static CURRENT_PLUGIN: RefCell<Option<String>> = const { RefCell::new(None) };
    static CALLBACK_DEPTH: RefCell<u32> = const { RefCell::new(0) };
}

/// Maximum synchronous callback depth (events, call_native, timers).
pub const MAX_CALLBACK_DEPTH: u32 = 5;

/// Engine-owned event names scripts must not fire via `fire_event`.
pub const RESERVED_SCRIPT_EVENTS: &[&str] = &["on_cvar_changed"];

/// Runs `f` with the trusted plugin identity set for native API checks.
pub fn with_plugin_context<T>(plugin_name: &str, f: impl FnOnce() -> T) -> T {
    CURRENT_PLUGIN.with(|cell| {
        *cell.borrow_mut() = Some(plugin_name.to_string());
        let out = f();
        *cell.borrow_mut() = None;
        out
    })
}

/// Returns the active plugin name if inside `with_plugin_context`.
pub fn current_plugin() -> Option<String> {
    CURRENT_PLUGIN.with(|cell| cell.borrow().clone())
}

/// Returns the active plugin name, defaulting to "unknown".
pub fn current_plugin_name() -> String {
    current_plugin().unwrap_or_else(|| "unknown".to_string())
}

/// Increments callback depth; returns Err if over budget.
pub struct CallbackGuard;

impl Drop for CallbackGuard {
    fn drop(&mut self) {
        leave_callback();
    }
}

/// Increments the thread-local callback depth counter and returns an RAII guard.
/// Returns `Err` if the recursion limit ([`MAX_CALLBACK_DEPTH`]) is exceeded.
pub fn enter_callback() -> Result<CallbackGuard, String> {
    CALLBACK_DEPTH.with(|cell| {
        let mut d = cell.borrow_mut();
        *d += 1;
        if *d > MAX_CALLBACK_DEPTH {
            Err("Callback recursion limit exceeded.".into())
        } else {
            Ok(CallbackGuard)
        }
    })
}

/// Decrements the thread-local callback depth counter (saturating at zero).
pub fn leave_callback() {
    CALLBACK_DEPTH.with(|cell| {
        let mut d = cell.borrow_mut();
        *d = d.saturating_sub(1);
    });
}

/// Verifies `claimed_plugin` matches `CURRENT_PLUGIN` unless cross-plugin FS is granted.
pub fn verify_plugin_identity(
    claimed_plugin: &str,
    caps: &CapabilitySet,
    allow_cross: Capability,
) -> Result<(), String> {
    let Some(current) = current_plugin() else {
        return Ok(());
    };
    if current == claimed_plugin {
        return Ok(());
    }
    if caps.has(allow_cross) {
        return Ok(());
    }
    Err(format!(
        "Plugin '{current}' cannot act as '{claimed_plugin}' (missing capability '{}').",
        allow_cross.as_str()
    ))
}

/// Per-plugin capability registry (populated at plugin load).
pub type PluginCapabilities = Arc<RwLock<HashMap<String, CapabilitySet>>>;

/// Creates an empty capability registry for initializing the engine.
pub fn empty_plugin_capabilities() -> PluginCapabilities {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Returns the capability set for the given plugin, or an empty default if not found.
pub fn capabilities_for(registry: &PluginCapabilities, plugin_name: &str) -> CapabilitySet {
    registry
        .read()
        .ok()
        .and_then(|m| m.get(plugin_name).cloned())
        .unwrap_or_default()
}

/// Asserts that the current plugin holds the given capability, or returns an error.
pub fn require_capability(registry: &PluginCapabilities, cap: Capability) -> Result<(), String> {
    let plugin = current_plugin_name();
    let caps = capabilities_for(registry, &plugin);
    caps.require(cap)
}

/// KV/global key namespacing: prefix unless `shared_global` granted.
pub fn namespaced_key(key: &str, caps: &CapabilitySet) -> String {
    if caps.has(Capability::SharedGlobal) || key.starts_with("shared::") {
        return key.to_string();
    }
    if let Some(plugin) = current_plugin() {
        format!("{plugin}::{key}")
    } else {
        key.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_mismatch_denied() {
        with_plugin_context("a", || {
            let caps = CapabilitySet::default();
            assert!(verify_plugin_identity("b", &caps, Capability::FsCrossPlugin).is_err());
        });
    }
}
