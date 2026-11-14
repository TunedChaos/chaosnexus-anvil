#[tokio::test]
async fn test_rhai_scripts() {
    // Setup env
    let test_dir = "tests";

    // Create PluginManager pointing to `tests`
    let manager = chaosnexus_anvil::scripting::manager::PluginManager::new(test_dir, None, None, std::sync::Arc::new(chaosnexus_anvil::config::Config::default()));

    // We expect the plugins in `tests/plugins` to run their `on_plugin_start` hooks.
    // If we want to actually execute specific test functions, we could use the manager
    // to call `test_main()` on them or similar.
    // But since `PluginManager::new` already compiles and calls `on_plugin_start`,
    // any `assert` calls in `on_plugin_start` will panic if they fail.

    // Wait for a second to allow any async tests to run (not ideal but works for simple cases)
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    manager.stop_all();
}
