// chaosnexus-anvil/tests/create_plugin_tests.rs
//
// Integration tests for plugin scaffolding and the built-in create-plugin path.

use chaosnexus_anvil::scripting::manager::PluginManager;
use chaosnexus_anvil::scripting::paths;
use chaosnexus_anvil::scripting::scaffold::{
    self, ScaffoldOptions, default_input_schema, entry_script_name, validate_plugin_name,
    validate_tool_name,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static TEST_ROOT: OnceLock<PathBuf> = OnceLock::new();
static TEST_SERIAL: Mutex<()> = Mutex::new(());

fn shared_scripts_root() -> &'static Path {
    TEST_ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("cw_scaffold_shared_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("plugins")).expect("create plugins dir");
        fs::create_dir_all(root.join("lib")).expect("create lib dir");
        fs::create_dir_all(root.join(".pending")).expect("create pending dir");
        paths::init(&root);
        root
    })
}

fn with_test_lock<F: FnOnce()>(f: F) {
    let guard = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    f();
    drop(guard);
}

fn cleanup_plugin(plugin_name: &str) {
    let dir = shared_scripts_root().join("plugins").join(plugin_name);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn scaffold_writes_canonical_plugin_layout() {
    with_test_lock(|| {
        let plugin_name = "test_plugin_success";
        cleanup_plugin(plugin_name);

        let result = scaffold::scaffold_plugin(ScaffoldOptions {
            plugin_name: plugin_name.to_string(),
            tool_name: "test_tool".to_string(),
            description: "A test plugin".to_string(),
            input_schema: None,
            script_body: None,
            custom_toml: None,
            overwrite: false,
        })
        .expect("scaffold should succeed");

        assert_eq!(
            result.script_path,
            shared_scripts_root()
                .join("plugins")
                .join(plugin_name)
                .join(entry_script_name(plugin_name))
        );
        assert!(result.toml_path.is_file());
        assert!(result.script_path.is_file());

        let script = fs::read_to_string(&result.script_path).expect("read script");
        assert!(script.contains("register_mcp_tool"));
        assert!(script.contains("fn execute(tool_name, args)"));
        assert!(script.contains("test_tool"));

        cleanup_plugin(plugin_name);
    });
}

#[test]
fn scaffold_rejects_invalid_names_and_existing_folder() {
    with_test_lock(|| {
        assert!(validate_plugin_name("disabled").is_err());
        assert!(validate_plugin_name("BadName").is_err());
        assert!(validate_tool_name("cn_a_reload_plugins").is_err());
        assert!(validate_tool_name("cn_a_disable_plugin").is_err());

        cleanup_plugin("dup_test");
        scaffold::scaffold_plugin(ScaffoldOptions {
            plugin_name: "dup_test".to_string(),
            tool_name: "dup_test_run".to_string(),
            description: "Test description that is long enough".to_string(),
            input_schema: None,
            script_body: None,
            custom_toml: None,
            overwrite: false,
        })
        .expect("scaffold");

        assert!(
            scaffold::scaffold_plugin(ScaffoldOptions {
                plugin_name: "dup_test".to_string(),
                tool_name: "dup_test_run".to_string(),
                description: "Test description that is long enough".to_string(),
                input_schema: None,
                script_body: None,
                custom_toml: None,
                overwrite: false,
            })
            .is_err(),
            "duplicate without overwrite must fail"
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reload_picks_up_scaffolded_plugin_tool() {
    let _guard = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let plugin_name = "echo_reload";
    let tool_name = "echo_reload_ping";
    cleanup_plugin(plugin_name);

    scaffold::scaffold_plugin(ScaffoldOptions {
        plugin_name: plugin_name.to_string(),
        tool_name: tool_name.to_string(),
        description: "Echo test plugin description".to_string(),
        input_schema: Some(default_input_schema().to_string()),
        script_body: None,
        custom_toml: None,
        overwrite: false,
    })
    .expect("scaffold");

    let pm = PluginManager::new(shared_scripts_root().to_string_lossy().as_ref(), None, None, std::sync::Arc::new(chaosnexus_anvil::config::Config::default()));
    let expected_tool_name = format!("cn_a_{}_{}", plugin_name, tool_name);
    assert!(
        pm.tool_exists(&expected_tool_name),
        "manager should have loaded and started the new plugin"
    );

    cleanup_plugin(plugin_name);
}

#[test]
fn scaffold_accepts_custom_script_body() {
    with_test_lock(|| {
        let plugin_name = "test_plugin_rhai_body";
        cleanup_plugin(plugin_name);

        let custom_script = r#"fn on_plugin_start() {}
fn execute(tool_name, args) { return "custom"; }
"#;

        scaffold::scaffold_plugin(ScaffoldOptions {
            plugin_name: plugin_name.to_string(),
            tool_name: "body_tool".to_string(),
            description: "Test custom rhai script".to_string(),
            input_schema: None,
            script_body: Some(custom_script.to_string()),
            custom_toml: None,
            overwrite: false,
        })
        .expect("Scaffold failed");

        let script_path = shared_scripts_root()
            .join("plugins")
            .join(plugin_name)
            .join(entry_script_name(plugin_name));
        let written = fs::read_to_string(&script_path).expect("read script");
        assert_eq!(written, custom_script);

        cleanup_plugin(plugin_name);
    });
}
