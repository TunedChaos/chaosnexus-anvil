use crate::scripting::capabilities::Capability;
use crate::scripting::models::NativeContext;
use crate::scripting::native_api::gates::require_cap;
use crate::scripting::plugin_context::{capabilities_for, namespaced_key};
use rhai::Engine;

/// Registers Key-Value (KV) store native functions with the Rhai engine.
pub fn register(engine: &mut Engine, n_ctx: &NativeContext) {
    let ctx = n_ctx.clone();
    engine.register_fn(
        "kv_get",
        move |key: &str| -> Result<rhai::Dynamic, Box<rhai::EvalAltResult>> {
            let plugin = crate::scripting::plugin_context::current_plugin()
                .unwrap_or_else(|| "unknown".to_string());
            let caps = capabilities_for(&ctx.plugin_capabilities, &plugin);
            let ns_key = namespaced_key(key, &caps);
            let store_guard = ctx.kv_store.lock().unwrap();
            let store = store_guard.as_ref().ok_or("KV store is not initialized")?;
            match store.get(&ns_key) {
                Ok(Some(val)) => Ok(val.into()),
                Ok(None) => Ok(rhai::Dynamic::UNIT),
                Err(e) => Err(e.into()),
            }
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "kv_set",
        move |key: &str, value: &str| -> Result<(), Box<rhai::EvalAltResult>> {
            let plugin = crate::scripting::plugin_context::current_plugin()
                .unwrap_or_else(|| "unknown".to_string());
            let caps = capabilities_for(&ctx.plugin_capabilities, &plugin);
            let ns_key = namespaced_key(key, &caps);
            let store_guard = ctx.kv_store.lock().unwrap();
            let store = store_guard.as_ref().ok_or("KV store is not initialized")?;
            store.set(&ns_key, value).map_err(|e| e.into())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "kv_dump",
        move || -> Result<String, Box<rhai::EvalAltResult>> {
            require_cap(&ctx, Capability::KvDump)?;
            let store_guard = ctx.kv_store.lock().unwrap();
            let store = store_guard.as_ref().ok_or("KV store is not initialized")?;
            store.dump().map_err(|e| e.into())
        },
    );
}
