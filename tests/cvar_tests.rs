// chaosnexus-anvil/tests/cvar_tests.rs
use chaosnexus_anvil::scripting::engine::setup_engine;
use chaosnexus_anvil::scripting::models::{CVar, NativeContext};
use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, RwLock};

fn create_test_context() -> NativeContext {
    let mut ctx = chaosnexus_anvil::scripting::engine::empty_context();
    // Ensure we have a cvars registry
    ctx.cvars = Arc::new(RwLock::new(HashMap::new()));
    ctx
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_register_and_get_cvar() {
    let ctx = create_test_context();
    // Simulate setting the global context for CONFIG injection and cvars access
    chaosnexus_anvil::scripting::models::GLOBAL_CONTEXT
        .set(ctx.clone())
        .ok();

    let engine = setup_engine(ctx.clone());
    let mut scope = chaosnexus_anvil::scripting::config_inject::build_plugin_scope("test_plugin");

    // Register a cvar
    engine
        .eval_with_scope::<()>(
            &mut scope,
            r#"register_cvar(PLUGIN_NAME, "my_var", "default_val", "desc");"#,
        )
        .expect("register cvar should succeed");

    // Retrieve via get_cvar
    let val = engine
        .eval_with_scope::<String>(&mut scope, r#"get_cvar("my_var")"#)
        .expect("get cvar should succeed");
    assert_eq!(val, "default_val");

    // Verify it is stored in the context correctly
    let cvars = ctx.cvars.read().unwrap();
    let cvar = cvars.get("my_var").expect("cvar exists in context");
    assert_eq!(cvar.plugin_name, "test_plugin");
    assert_eq!(cvar.value, "default_val");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_cvars_toml_override_applies() {
    let ctx = create_test_context();
    chaosnexus_anvil::scripting::models::GLOBAL_CONTEXT
        .set(ctx.clone())
        .ok();

    let engine = setup_engine(ctx.clone());
    let mut scope = chaosnexus_anvil::scripting::config_inject::build_plugin_scope("test_plugin");

    // Register a cvar with default value
    engine
        .eval_with_scope::<()>(
            &mut scope,
            r#"register_cvar(PLUGIN_NAME, "api_key", "default_key", "API Key");"#,
        )
        .expect("register cvar");

    // Create a temporary cvars.toml
    let tmp_dir = std::env::temp_dir().join(format!("cw_cvars_test_{}", std::process::id()));
    fs::create_dir_all(&tmp_dir).unwrap();
    let cvars_file = tmp_dir.join("cvars.toml");
    fs::write(
        &cvars_file,
        r#"
        [cvars]
        api_key = "overridden_key"
    "#,
    )
    .unwrap();

    // Load overrides
    let overrides = chaosnexus_anvil::scripting::cvars::load_overrides(&tmp_dir);
    {
        let mut c = ctx.cvars.write().unwrap();
        for (name, value) in overrides {
            c.insert(
                name.clone(),
                CVar {
                    plugin_name: String::new(),
                    name,
                    value,
                    description: String::new(),
                },
            );
        }
    }

    // Retrieve the overridden value via get_cvar
    let val = engine
        .eval_with_scope::<String>(&mut scope, r#"get_cvar("api_key")"#)
        .expect("get cvar");

    assert_eq!(val, "overridden_key");

    let _ = fs::remove_dir_all(&tmp_dir);
}
