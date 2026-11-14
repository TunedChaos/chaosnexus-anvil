// chaosnexus-anvil/tests/e2e_plugins_tests.rs
//
// End-to-end integration tests for the live PluginManager load path.
// Public scripts root ships `translation_test` only; terminal (register_mcp_tool +
// load_config) lives under Forge fixtures after the Suite packaging prune.

use chaosnexus_anvil::config::{Config, Permissions, PluginConfig};
use chaosnexus_anvil::scripting::manager::PluginManager;
use rust_mcp_sdk::schema::CallToolRequestParams;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Resolve the slim public scripts polyrepo (`translation_test` only).
fn resolve_scripts_dir() -> PathBuf {
    let cwd = std::env::current_dir().unwrap();
    if cwd.join("../chaosnexus-scripts/plugins").exists() {
        cwd.join("../chaosnexus-scripts")
    } else if cwd.join("chaosnexus-scripts/plugins").exists() {
        cwd.join("chaosnexus-scripts")
    } else {
        panic!(
            "Could not locate chaosnexus-scripts/plugins directory from {:?}",
            cwd
        );
    }
}

/// Resolve Forge fixture scripts (includes `terminal` for registry smoke).
fn resolve_forge_fixtures_scripts_dir() -> PathBuf {
    let cwd = std::env::current_dir().unwrap();
    let candidates = [
        cwd.join("../chaosnexus-forge/fixtures/scripts"),
        cwd.join("chaosnexus-forge/fixtures/scripts"),
    ];
    for path in &candidates {
        if path.join("plugins/terminal").exists() {
            return path.clone();
        }
    }
    panic!(
        "Could not locate chaosnexus-forge/fixtures/scripts/plugins/terminal from {:?}",
        cwd
    );
}

fn terminal_host_config() -> Config {
    let mut config = Config::default();
    let mut plugins = HashMap::new();
    plugins.insert(
        "terminal".to_string(),
        PluginConfig {
            permissions: Some(Permissions {
                shell: Some(vec![
                    "sh".to_string(),
                    "bash".to_string(),
                    "zsh".to_string(),
                    "powershell".to_string(),
                    "echo".to_string(),
                ]),
                ..Default::default()
            }),
            granted_capabilities: Some(vec!["shell".to_string(), "env".to_string()]),
            env_allowlist: Some(vec!["SHELL".to_string()]),
            secrets: None,
        },
    );
    config.plugins = Some(plugins);
    config
}

/// Live path: `load_config` (2-arg) + `register_mcp_tool` (4-arg) during `on_plugin_start`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_live_register_mcp_tool_and_load_config() {
    let scripts_dir = resolve_forge_fixtures_scripts_dir();
    let config = Arc::new(terminal_host_config());

    let pm = PluginManager::new(scripts_dir.to_str().unwrap(), None, None, config);

    assert!(
        pm.tool_exists("cn_a_terminal_test_echo"),
        "terminal on_plugin_start must register test_echo via register_mcp_tool + load_config (no Function not found)"
    );

    let terminal_params = CallToolRequestParams {
        name: "cn_a_terminal_test_echo".to_string(),
        arguments: Some(
            serde_json::json!({})
                .as_object()
                .unwrap()
                .clone(),
        ),
        meta: None,
        task: None,
    };
    let terminal_res = pm
        .handle_tool("cn_a_terminal_test_echo", &terminal_params)
        .await
        .expect("handle_tool execution error")
        .expect("tool execution result missing");

    let terminal_text = serde_json::to_string(&terminal_res).unwrap();
    assert!(
        terminal_text.contains("Hello, Chaos!"),
        "terminal test_echo should execute echo and return 'Hello, Chaos!', got: {}",
        terminal_text
    );
}

/// Public scripts root still loads the example plugin without registry failures.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_public_scripts_translation_test_loads() {
    let scripts_dir = resolve_scripts_dir();
    let config = Arc::new(Config::default());

    let pm = PluginManager::new(scripts_dir.to_str().unwrap(), None, None, config);

    assert!(
        pm.plugin_loaded("translation_test"),
        "translation_test must complete on_plugin_start (translate + log_info overloads)"
    );
}
