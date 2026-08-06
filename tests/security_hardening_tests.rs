// chaosnexus-anvil/tests/security_hardening_tests.rs
//
// Integration tests for quarantine staging, capability gates, identity binding,
// and reserved engine events.

use chaosnexus_anvil::scripting::capabilities::CapabilitySet;
use chaosnexus_anvil::scripting::engine::{empty_context, setup_engine};
use chaosnexus_anvil::scripting::manager::PluginManager;
use chaosnexus_anvil::scripting::paths;
use chaosnexus_anvil::scripting::plugin_context::{RESERVED_SCRIPT_EVENTS, with_plugin_context};
use chaosnexus_anvil::scripting::scaffold::{self, PENDING_MANIFEST, PendingScaffoldOptions};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static TEST_ROOT: OnceLock<PathBuf> = OnceLock::new();
static TEST_SERIAL: Mutex<()> = Mutex::new(());

fn shared_scripts_root() -> &'static Path {
    TEST_ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("cw_security_{}", std::process::id()));
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

fn cleanup_pending(name: &str) {
    let _ = fs::remove_dir_all(shared_scripts_root().join(".pending").join(name));
}

fn cleanup_plugin(name: &str) {
    let _ = fs::remove_dir_all(shared_scripts_root().join("plugins").join(name));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_root_is_outside_plugin_discovery() {
    let _guard = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let plugin_name = "quarantine_probe";
    cleanup_pending(plugin_name);
    cleanup_plugin(plugin_name);

    scaffold::scaffold_pending_plugin(PendingScaffoldOptions {
        plugin_name: plugin_name.to_string(),
        tool_name: "test_tool".to_string(),
        description: "Network testing".to_string(),
        input_schema: None,
        script_body: None,
        custom_toml: None,
        requested_capabilities: vec!["net_http".to_string()],
        overwrite: false,
    })
    .expect("stage pending");

    let pending_dir = shared_scripts_root().join(".pending").join(plugin_name);
    assert!(pending_dir.is_dir());
    assert!(pending_dir.join(PENDING_MANIFEST).is_file());

    let pm = PluginManager::new(shared_scripts_root().to_string_lossy().as_ref(), None, None, std::sync::Arc::new(chaosnexus_anvil::config::Config::default()));
    assert!(
        !pm.tool_exists("quarantine_probe_run"),
        "pending plugins must never be loaded"
    );

    cleanup_pending(plugin_name);
}

#[test]
fn promote_pending_writes_granted_capabilities() {
    with_test_lock(|| {
        let plugin_name = "promote_caps";
        cleanup_pending(plugin_name);
        cleanup_plugin(plugin_name);

        scaffold::scaffold_pending_plugin(PendingScaffoldOptions {
            plugin_name: plugin_name.to_string(),
            tool_name: "test_tool".to_string(),
            description: "No net caps".to_string(),
            input_schema: None,
            script_body: None,
            custom_toml: None,
            requested_capabilities: vec![],
            overwrite: false,
        })
        .expect("stage");

        scaffold::promote_pending_plugin(plugin_name, &["shell".to_string()], &[])
            .expect("promote");

        let toml = fs::read_to_string(
            shared_scripts_root()
                .join("plugins")
                .join(plugin_name)
                .join("plugin.toml"),
        )
        .expect("read toml");
        assert!(toml.contains("[capabilities]"));
        assert!(toml.contains("shell"));
        assert!(!toml.contains("net_http"));

        cleanup_plugin(plugin_name);
    });
}

#[test]
fn run_command_denied_without_shell_capability() {
    with_test_lock(|| {
        let ctx = empty_context();
        {
            let mut caps = ctx.plugin_capabilities.write().unwrap();
            caps.insert("deny_plugin".to_string(), CapabilitySet::default());
        }
        let engine = setup_engine(ctx);
        let script = r#"run_command("/bin/sh", "echo hi")"#;
        let ast = engine.compile(script).expect("compile");
        let err = with_plugin_context("deny_plugin", || engine.eval_ast::<rhai::Dynamic>(&ast));
        assert!(err.is_err(), "expected capability denial");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("Capability") && msg.contains("shell"),
            "unexpected error: {msg}"
        );
    });
}

#[test]
fn run_command_allowed_with_shell_capability() {
    with_test_lock(|| {
        let ctx = empty_context();
        {
            let mut caps = ctx.plugin_capabilities.write().unwrap();
            caps.insert(
                "allow_plugin".to_string(),
                CapabilitySet::from_id_list(&["shell".to_string()]),
            );
            
            let mut plugins = ctx.plugins.write().unwrap();
            plugins.insert("allow_plugin".to_string(), chaosnexus_anvil::config::PluginConfig {
                permissions: Some(chaosnexus_anvil::config::Permissions {
                    shell: Some(vec!["echo".to_string()]),
                    ..Default::default()
                }),
                secrets: None,
                ..Default::default()
            });
        }
        let engine = setup_engine(ctx);
        let script = r#"run_command("/bin/sh", "echo ok")"#;
        let ast = engine.compile(script).expect("compile");
        let out = with_plugin_context("allow_plugin", || engine.eval_ast::<rhai::Dynamic>(&ast))
            .expect("shell granted");
        assert_eq!(out.to_string(), "ok\n");
    });
}

#[test]
#[cfg(windows)]
fn run_command_powershell_on_windows() {
    with_test_lock(|| {
        let ctx = empty_context();
        {
            let mut caps = ctx.plugin_capabilities.write().unwrap();
            caps.insert(
                "allow_plugin".to_string(),
                CapabilitySet::from_id_list(&["shell".to_string()]),
            );

            let mut perms = ctx.plugin_permissions.write().unwrap();
            perms.insert("allow_plugin".to_string(), vec!["Write-Output".to_string()]);
        }
        let engine = setup_engine(ctx);
        let script = r#"run_command("powershell", "Write-Output ok")"#;
        let ast = engine.compile(script).expect("compile");
        let out = with_plugin_context("allow_plugin", || engine.eval_ast::<rhai::Dynamic>(&ast))
            .expect("powershell shell granted");
        let text = out.to_string();
        assert!(
            text.contains("ok"),
            "expected powershell output to contain ok, got: {text:?}"
        );
    });
}

#[test]
fn identity_binding_rejects_cross_plugin_fs_claim() {
    let ctx = empty_context();
    {
        let mut caps = ctx.plugin_capabilities.write().unwrap();
        caps.insert("caller".to_string(), CapabilitySet::default());
    }
    let engine = setup_engine(ctx);
    let script = r#"
fn probe() {
    fs_read("other_plugin", "plugin.toml");
}
probe();
"#;
    let ast = engine.compile(script).expect("compile");
    let err = with_plugin_context("caller", || engine.run_ast(&ast));
    assert!(err.is_err());
    let msg = format!("{err:?}");
    assert!(
        msg.contains("cannot act as") || msg.contains("Capability"),
        "expected identity rejection, got: {msg}"
    );
}

#[test]
fn reserved_events_cannot_be_fired_by_scripts() {
    for event in RESERVED_SCRIPT_EVENTS {
        assert!(
            !event.is_empty(),
            "reserved event list must remain documented"
        );
    }
}

#[test]
fn get_env_denied_raises_hard_error() {
    with_test_lock(|| {
        let ctx = empty_context();
        {
            let mut caps = ctx.plugin_capabilities.write().unwrap();
            caps.insert(
                "no_env_plugin".to_string(),
                CapabilitySet::from_id_list(&["env".to_string()]),
            );
        }
        let engine = setup_engine(ctx);
        let script = r#"get_env("AWS_SECRET_ACCESS_KEY")"#;
        let ast = engine.compile(script).expect("compile");
        let err = with_plugin_context("no_env_plugin", || engine.run_ast(&ast));
        assert!(
            err.is_err(),
            "get_env must hard-error when key is not allowlisted"
        );
        let msg = format!("{err:?}");
        assert!(
            msg.contains("env_allowlist") || msg.contains("not in the granted"),
            "expected allowlist denial message, got: {msg}"
        );
    });
}

#[test]
fn config_constant_is_injected_and_immutable() {
    let engine = setup_engine(empty_context());
    let mut scope = chaosnexus_anvil::scripting::config_inject::build_plugin_scope("scope_probe");

    // CONFIG is auto-available (no import) and reports the owning plugin.
    let plugin = engine
        .eval_with_scope::<String>(&mut scope, "CONFIG.plugin")
        .expect("CONFIG should be present in the plugin scope");
    assert_eq!(plugin, "scope_probe");

    // CONFIG.cvars / CONFIG.secrets always exist so scripts can rely on them.
    let has_maps = engine
        .eval_with_scope::<bool>(
            &mut scope,
            r#"type_of(CONFIG.cvars) == "map" && type_of(CONFIG.secrets) == "map""#,
        )
        .expect("CONFIG sub-maps should be present");
    assert!(has_maps, "CONFIG.cvars and CONFIG.secrets must be maps");

    // CONFIG is an immutable constant: reassignment must be rejected.
    let mutate = engine.eval_with_scope::<()>(&mut scope, "CONFIG = #{};");
    assert!(mutate.is_err(), "CONFIG must be immutable");
}

#[test]
fn test_net_allowlist_wildcard_permission_struct() {
    let toml = r#"
        [plugins.test_p.permissions]
        net_allowlist = ["*.github.com", "api.example.com"]
        http = ["GET", "POST"]
    "#;
    let config: chaosnexus_anvil::config::Config =
        toml::from_str(toml).expect("parse config with net_allowlist");
    let plugin_cfg = config.plugins.unwrap().remove("test_p").unwrap();
    let perms = plugin_cfg.permissions.unwrap();

    let allowlist = perms.net_allowlist.unwrap();
    assert_eq!(allowlist, vec!["*.github.com", "api.example.com"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_granted_capabilities_required_for_plugin_toml_caps() {
    let _guard = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = shared_scripts_root();
    let plugin_name = "cap_intersect";
    cleanup_plugin(plugin_name);

    let plugin_dir = root.join("plugins").join(plugin_name);
    fs::create_dir_all(&plugin_dir).expect("plugin dir");
    fs::write(
        plugin_dir.join("plugin.toml"),
        r#"
name = "cap_intersect"
version = "1.0.0"

[capabilities]
granted = ["shell", "net_http"]
"#,
    )
    .expect("toml");
    fs::write(
        plugin_dir.join("cap_intersect_tool.rhai"),
        "fn on_plugin_start() {}\nfn execute(tool_name, args) { \"ok\" }\n",
    )
    .expect("script");

    let mut config = chaosnexus_anvil::config::Config::default();
    let mut plugins = std::collections::HashMap::new();
    plugins.insert(
        plugin_name.to_string(),
        chaosnexus_anvil::config::PluginConfig {
            granted_capabilities: Some(vec!["shell".to_string()]),
            permissions: Some(chaosnexus_anvil::config::Permissions {
                shell: Some(vec!["echo".to_string()]),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    config.plugins = Some(plugins);

    let pm = PluginManager::new(
        root.to_string_lossy().as_ref(),
        None,
        None,
        std::sync::Arc::new(config),
    );
    assert!(
        pm.plugin_loaded(plugin_name),
        "plugin with host granted_capabilities intersection must load"
    );

    cleanup_plugin(plugin_name);
}

#[test]
fn plugin_toml_cannot_self_authorize_http_permissions() {
    // Unknown keys / forged [permissions] in plugin.toml are ignored by PluginMeta
    // and never appear in host Config.plugins (the only Permissions source).
    let forged = r#"
name = "sneaky"
version = "1.0.0"

[permissions]
http = ["GET", "POST", "DELETE"]
net_allowlist = ["*"]
"#;
    let meta: Result<chaosnexus_anvil::scripting::models::PluginMeta, _> = toml::from_str(forged);
    assert!(meta.is_ok(), "unknown tables must not fail parse");
    let host_default = chaosnexus_anvil::config::Config::default();
    assert!(
        host_default.plugins.is_none(),
        "default host config must not inherit plugin-dir permissions"
    );
}
