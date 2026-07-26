// chaosnexus-anvil/src/scripting/engine.rs
use crate::scripting::lib_module_resolver::SharedLibModuleResolver;
use crate::scripting::models::NativeContext;
use crate::scripting::paths;
use rhai::Engine;
use rhai::module_resolvers::{FileModuleResolver, ModuleResolversCollection};

// NOTE: `write_log` lives in `manager.rs`. Plugin path helpers live in
// `paths.rs` (single source of truth for `plugins/` and `lib/` roots).

/// Registers module resolvers so Rhai scripts can `import` plugin-local helpers
/// next to the loading script and shared libraries under `scripts/lib/` only.
fn configure_module_resolvers(engine: &mut Engine) {
    let mut resolvers = ModuleResolversCollection::new();
    // Plugin-local: `import "helpers" as h;` relative to the loading `.rhai` file.
    resolvers.push(FileModuleResolver::new());
    // `import "lib/foo" as f;` -> `<scripts_root>/lib/foo.rhai` (no other roots).
    resolvers.push(SharedLibModuleResolver::new(paths::lib_root()));
    engine.set_module_resolver(resolvers);
}

/// Configures a fresh Rhai engine with sandboxing limits and registers all native API modules.
pub fn setup_engine(context: NativeContext) -> Engine {
    let mut engine = Engine::new();
    configure_module_resolvers(&mut engine);
    
    // Register extended math and data science capabilities
    rhai::packages::Package::register_into_engine(&rhai_rand::RandomPackage::new(), &mut engine);
    rhai::packages::Package::register_into_engine(&rhai_sci::SciPackage::new(), &mut engine);
    rhai::packages::Package::register_into_engine(&rhai_ml::MLPackage::new(), &mut engine);
    rhai::packages::Package::register_into_engine(&rhai_bigint::BigIntPackage::new(), &mut engine);
    engine.set_max_operations(2_000_000);
    engine.set_max_call_levels(64);
    engine.set_max_string_size(1_048_576);
    engine.set_max_array_size(65_536);
    engine.set_max_map_size(65_536);
    engine.disable_symbol("eval");
    engine.on_print(|x| crate::scripting::manager::write_log("Rhai", "INFO", x));
    engine.on_debug(|x, src, pos| {
        crate::scripting::manager::write_log(
            "Rhai",
            "DEBUG",
            &format!("{} @ {:?} {:?}", x, src, pos),
        )
    });

    crate::scripting::native_api::register_all(&mut engine, &context);
    engine
}

/// Builds a fully-initialized but state-free [`NativeContext`].
///
/// All registries are empty and there is no live KV store, timer task, or MCP
/// logging channel. It is suitable for schema extraction and for tests that
/// only need a working engine (e.g. exercising the outbound MCP client native
/// functions) without spinning up the full plugin manager.
pub fn empty_context() -> NativeContext {
    NativeContext {
        asts: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        tools: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        tool_owners: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        db_connections: std::sync::Arc::new(
            std::sync::Mutex::new(std::collections::HashMap::new()),
        ),
        events: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        global_state: std::sync::Arc::new(std::sync::RwLock::new(std::collections::HashMap::new())),
        cvars: std::sync::Arc::new(std::sync::RwLock::new(std::collections::HashMap::new())),
        plugins: std::sync::Arc::new(std::sync::RwLock::new(std::collections::HashMap::new())),
        natives: std::sync::Arc::new(std::sync::RwLock::new(std::collections::HashMap::new())),
        ws_handles: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        webhook_handles: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        webhook_routes: std::sync::Arc::new(std::sync::RwLock::new(
            std::collections::HashMap::new(),
        )),
        timer_tx: tokio::sync::mpsc::unbounded_channel().0,
        kv_store: std::sync::Arc::new(std::sync::Mutex::new(None)),
        resources: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        resource_owners: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        prompts: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        prompt_owners: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        reload_requested: std::sync::Arc::new(std::sync::Mutex::new(false)),
        mcp_log_tx: None,
        translations: std::sync::Arc::new(std::sync::RwLock::new(std::collections::HashMap::new())),
        mcp_clients: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        trace_store: std::sync::Arc::new(std::sync::Mutex::new(
            crate::scripting::trace::TraceStore::new(256),
        )),
        active_trace: std::sync::Arc::new(std::sync::Mutex::new(None)),
        plugin_capabilities: crate::scripting::plugin_context::empty_plugin_capabilities(),
        plugin_prefixes: std::sync::Arc::new(std::sync::RwLock::new(
            std::collections::HashMap::new(),
        )),
        ide_connection_info: None,
    }
}

/// Generates the documented SSOT schema for every registered native function.
///
/// The raw Rhai metadata (a flat `functions` array) is transformed into the
/// richer `modules[]` contract consumed by ChaosNexus Forge's Monaco providers and
/// the VitePress documentation generator. See [`crate::scripting::schema`].
pub fn generate_system_schema() -> String {
    // Context strictly for schema extraction (no real references needed for docstrings).
    let engine = setup_engine(empty_context());
    let raw = engine
        .gen_fn_metadata_to_json(false)
        .expect("Failed to serialize runtime engine schema");
    crate::scripting::schema::transform_metadata(&raw)
}
