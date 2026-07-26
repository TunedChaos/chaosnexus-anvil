// chaosnexus-anvil/tests/disable_plugin_tests.rs
//
// Integration tests for `chaoswrench_disable_plugin` folder moves and discovery skip.

use chaosnexus_anvil::scripting::manager::PluginManager;
use chaosnexus_anvil::scripting::paths;
use chaosnexus_anvil::scripting::scaffold::{self, ScaffoldOptions, entry_script_name};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static TEST_ROOT: OnceLock<PathBuf> = OnceLock::new();
static TEST_SERIAL: Mutex<()> = Mutex::new(());

fn shared_scripts_root() -> &'static Path {
    TEST_ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("cw_disable_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("plugins")).expect("plugins");
        fs::create_dir_all(root.join("lib")).expect("lib");
        fs::create_dir_all(root.join(".pending")).expect("pending");
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
    let _ = fs::remove_dir_all(shared_scripts_root().join("plugins").join(plugin_name));
    let _ = fs::remove_dir_all(
        shared_scripts_root()
            .join("plugins")
            .join("disabled")
            .join(plugin_name),
    );
}

#[test]
fn disable_plugin_moves_folder_under_plugins_disabled() {
    with_test_lock(|| {
        let plugin_name = "test_plugin_disable";
        cleanup_plugin(plugin_name);

        scaffold::scaffold_plugin(ScaffoldOptions {
            plugin_name: "test_plugin_disable".to_string(),
            tool_name: "test_tool".to_string(),
            description: "Plugin to be disabled".to_string(),
            input_schema: None,
            script_body: None,
            custom_toml: None,
            overwrite: true,
        })
        .expect("Scaffold failed");

        let live = shared_scripts_root().join("plugins").join(plugin_name);
        assert!(live.is_dir());

        let dest = scaffold::disable_plugin(plugin_name).expect("disable");
        assert_eq!(
            dest,
            shared_scripts_root()
                .join("plugins")
                .join("disabled")
                .join(plugin_name)
        );
        assert!(!live.exists());
        assert!(dest.join("plugin.toml").is_file());
        assert!(dest.join(entry_script_name(plugin_name)).is_file());

        let err = scaffold::disable_plugin(plugin_name).unwrap_err();
        assert!(err.contains("already disabled"));

        cleanup_plugin(plugin_name);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_plugins_are_not_discovered() {
    let _guard = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let plugin_name = "test_plugin_disable_invalid";
    cleanup_plugin(plugin_name);

    scaffold::scaffold_plugin(ScaffoldOptions {
        plugin_name: "test_plugin_disable_invalid".to_string(),
        tool_name: "test_tool".to_string(),
        description: "Plugin to be disabled but state is invalid".to_string(),
        input_schema: None,
        script_body: None,
        custom_toml: None,
        overwrite: true,
    })
    .expect("Scaffold failed");

    let pm = PluginManager::new(shared_scripts_root().to_string_lossy().as_ref(), None, None, std::sync::Arc::new(chaosnexus_anvil::config::Config::default()));
    assert!(pm.tool_exists("cn_a_test_plugin_disable_invalid_test_tool"));

    scaffold::disable_plugin(plugin_name).expect("disable");

    let pm = PluginManager::new(shared_scripts_root().to_string_lossy().as_ref(), None, None, std::sync::Arc::new(chaosnexus_anvil::config::Config::default()));
    assert!(
        !pm.tool_exists("cn_a_test_plugin_disable_invalid_test_tool"),
        "disabled plugins must not register tools"
    );

    cleanup_plugin(plugin_name);
}
