// chaosnexus-anvil/tests/e2e_plugins_tests.rs
//
// End-to-end integration tests verifying live native plugins (time, terminal, mcp_bridge_demo)
// loaded from the project's `chaosnexus-scripts/` directory.

use chaosnexus_anvil::config::{Config, Permissions, PluginConfig};
use chaosnexus_anvil::scripting::manager::PluginManager;
use rust_mcp_sdk::schema::CallToolRequestParams;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn resolve_scripts_dir() -> PathBuf {
    let cwd = std::env::current_dir().unwrap();
    if cwd.join("../chaosnexus-scripts/plugins").exists() {
        cwd.join("../chaosnexus-scripts")
    } else if cwd.join("chaosnexus-scripts/plugins").exists() {
        cwd.join("chaosnexus-scripts")
    } else {
        panic!("Could not locate chaosnexus-scripts/plugins directory from {:?}", cwd);
    }
}

fn build_e2e_config() -> Config {
    let mut config = Config::default();
    let mut plugins = HashMap::new();

    // Grant shell capability and allowlist for terminal plugin
    plugins.insert(
        "terminal".to_string(),
        PluginConfig {
            permissions: Some(Permissions {
                shell: Some(vec!["sh".to_string(), "bash".to_string(), "zsh".to_string(), "powershell".to_string(), "echo".to_string()]),
                ..Default::default()
            }),
            granted_capabilities: Some(vec!["shell".to_string(), "env".to_string()]),
            env_allowlist: Some(vec!["SHELL".to_string()]),
            secrets: None,
        },
    );

    // Grant net_http capability for time plugin
    plugins.insert(
        "time".to_string(),
        PluginConfig {
            granted_capabilities: Some(vec!["net_http".to_string()]),
            ..Default::default()
        },
    );

    // Grant process_spawn capability for mcp_bridge_demo plugin
    plugins.insert(
        "mcp_bridge_demo".to_string(),
        PluginConfig {
            granted_capabilities: Some(vec!["process_spawn".to_string()]),
            ..Default::default()
        },
    );

    config.plugins = Some(plugins);
    config
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_e2e_native_plugins_registration_and_execution() {
    let scripts_dir = resolve_scripts_dir();
    let config = Arc::new(build_e2e_config());

    let pm = PluginManager::new(
        scripts_dir.to_str().unwrap(),
        None,
        None,
        config,
    );

    // 1. Verify Registration of Native Tools
    assert!(
        pm.tool_exists("cn_a_time_get_system_time"),
        "time plugin tool 'cn_a_time_get_system_time' must be registered"
    );
    assert!(
        pm.tool_exists("cn_a_time_get_ntp_time"),
        "time plugin tool 'cn_a_time_get_ntp_time' must be registered"
    );
    assert!(
        pm.tool_exists("cn_a_terminal_test_echo"),
        "terminal plugin tool 'cn_a_terminal_test_echo' must be registered"
    );
    assert!(
        pm.tool_exists("cn_a_mcp_bridge_demo_mcp_bridge_probe"),
        "mcp_bridge_demo plugin tool 'cn_a_mcp_bridge_demo_mcp_bridge_probe' must be registered"
    );
    assert!(
        pm.tool_exists("cn_a_mcp_bridge_demo_mcp_bridge_relay"),
        "mcp_bridge_demo plugin tool 'cn_a_mcp_bridge_demo_mcp_bridge_relay' must be registered"
    );

    // 2. Execute 'cn_a_time_get_system_time'
    let time_params = CallToolRequestParams {
        name: "cn_a_time_get_system_time".to_string(),
        arguments: Some(serde_json::json!({ "timezone": "UTC" }).as_object().unwrap().clone()),
        meta: None,
        task: None,
    };
    let time_res = pm
        .handle_tool("cn_a_time_get_system_time", &time_params)
        .await
        .expect("handle_tool execution error")
        .expect("tool execution result missing");

    let time_text = serde_json::to_string(&time_res).unwrap();
    assert!(
        !time_text.is_empty(),
        "get_system_time should return system timestamp output"
    );

    // 3. Execute 'cn_a_terminal_test_echo'
    let terminal_params = CallToolRequestParams {
        name: "cn_a_terminal_test_echo".to_string(),
        arguments: Some(serde_json::json!({}).as_object().unwrap().clone()),
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

    // 4. Execute 'cn_a_mcp_bridge_demo_mcp_bridge_probe' via outbound MCP stdio
    let bin = env!("CARGO_BIN_EXE_chaosnexus-anvil");
    let probe_params = CallToolRequestParams {
        name: "cn_a_mcp_bridge_demo_mcp_bridge_probe".to_string(),
        arguments: Some(serde_json::json!({
            "command": bin,
            "args": []
        }).as_object().unwrap().clone()),
        meta: None,
        task: None,
    };
    let probe_res = pm
        .handle_tool("cn_a_mcp_bridge_demo_mcp_bridge_probe", &probe_params)
        .await
        .expect("probe tool execution error")
        .expect("probe tool result missing");

    let probe_text = serde_json::to_string(&probe_res).unwrap();
    assert!(
        probe_text.contains("Downstream tools:"),
        "mcp_bridge_probe should list downstream tools, got: {}",
        probe_text
    );
}
