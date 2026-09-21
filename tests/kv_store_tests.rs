// chaosnexus-anvil/tests/kv_store_tests.rs
use chaosnexus_anvil::scripting::engine::setup_engine;
use chaosnexus_anvil::scripting::models::NativeContext;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

fn create_test_context() -> NativeContext {
    let mut ctx = chaosnexus_anvil::scripting::engine::empty_context();
    ctx.global_state = Arc::new(RwLock::new(HashMap::new()));
    ctx
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_set_and_get_global() {
    let ctx = create_test_context();
    chaosnexus_anvil::scripting::models::GLOBAL_CONTEXT
        .set(ctx.clone())
        .ok();

    let engine = setup_engine(ctx.clone());
    let mut scope = chaosnexus_anvil::scripting::config_inject::build_plugin_scope("test_plugin");

    // We must execute within plugin context so `current_plugin()` returns "test_plugin".
    chaosnexus_anvil::scripting::plugin_context::with_plugin_context("test_plugin", || {
        // Set a global variable
        engine
            .eval_with_scope::<()>(&mut scope, r#"set_global("my_data", 42);"#)
            .expect("set global");

        // Retrieve it
        let val = engine
            .eval_with_scope::<i64>(&mut scope, r#"get_global("my_data")"#)
            .expect("get global");

        assert_eq!(val, 42);
    });

    // Verify namespacing applies when reading from context directly
    let state = ctx.global_state.read().unwrap();
    // Since "test_plugin" lacks the `SharedGlobal` capability, the key should be namespaced.
    assert!(state.contains_key("test_plugin::my_data"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_cross_plugin_global_isolation() {
    let ctx = create_test_context();
    chaosnexus_anvil::scripting::models::GLOBAL_CONTEXT
        .set(ctx.clone())
        .ok();

    let engine = setup_engine(ctx.clone());

    // Plugin A sets a global
    chaosnexus_anvil::scripting::plugin_context::with_plugin_context("plugin_a", || {
        let mut scope = chaosnexus_anvil::scripting::config_inject::build_plugin_scope("plugin_a");
        engine
            .eval_with_scope::<()>(&mut scope, r#"set_global("shared_key", "secret_A");"#)
            .expect("plugin_a set global");
    });

    // Plugin B sets its own global under the same script-level key
    chaosnexus_anvil::scripting::plugin_context::with_plugin_context("plugin_b", || {
        let mut scope = chaosnexus_anvil::scripting::config_inject::build_plugin_scope("plugin_b");
        engine
            .eval_with_scope::<()>(&mut scope, r#"set_global("shared_key", "secret_B");"#)
            .expect("plugin_b set global");

        // Plugin B reads its own value
        let val_b = engine
            .eval_with_scope::<String>(&mut scope, r#"get_global("shared_key")"#)
            .expect("plugin_b get global");
        assert_eq!(val_b, "secret_B");
    });

    // Plugin A reads its own value and sees it is untouched
    chaosnexus_anvil::scripting::plugin_context::with_plugin_context("plugin_a", || {
        let mut scope = chaosnexus_anvil::scripting::config_inject::build_plugin_scope("plugin_a");
        let val_a = engine
            .eval_with_scope::<String>(&mut scope, r#"get_global("shared_key")"#)
            .expect("plugin_a get global");
        assert_eq!(val_a, "secret_A");
    });
}
