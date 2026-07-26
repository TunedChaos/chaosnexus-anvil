// chaosnexus-anvil/src/scripting/config_inject.rs
//
// Per-plugin `CONFIG` injection. Delivers a plugin's scoped configuration and
// granted secrets into the Rhai scope as an immutable global constant, so
// scripts never reach for ambient `get_env`/host state to authenticate or
// configure outbound calls.
//
// Security model: `CONFIG` is rebuilt on every plugin invocation under the
// trusted `CURRENT_PLUGIN` identity and contains ONLY:
//   * `CONFIG.cvars`   - cvars owned by (scoped to) the calling plugin
//   * `CONFIG.secrets` - host env vars the plugin was explicitly granted via
//                        its `env` capability + `env_allowlist`
//   * `CONFIG.plugin`  - the calling plugin's own name (convenience)
// A plugin can never observe another plugin's `CONFIG`. It is pushed as a Rhai
// constant (`push_constant`) so scripts cannot reassign it.

use crate::scripting::capabilities::{Capability, CapabilitySet};
use crate::scripting::models::{CVar, GLOBAL_CONTEXT};
use rhai::{Dynamic, Map, Scope};
use std::collections::HashMap;

/// Builds a fresh Rhai scope for `plugin_name` pre-loaded with the `PLUGIN_NAME`
/// identity and the immutable, per-plugin `CONFIG` constant.
///
/// This is the single source of truth for plugin scope construction so every
/// invocation path (lifecycle, tool/resource/prompt dispatch, events, timers)
/// gets identical, identity-scoped configuration.
pub fn build_plugin_scope(plugin_name: &str) -> Scope<'static> {
    let mut scope = Scope::new();
    scope.push("PLUGIN_NAME", plugin_name.to_string());
    scope.push_constant("CONFIG", build_config_map(plugin_name));
    scope
}

/// Assembles the `CONFIG` map for `plugin_name` from the live global context.
///
/// Falls back to an empty (but well-formed) `CONFIG` when no manager context is
/// initialized (e.g. lightweight engine tests), so scripts can always rely on
/// `CONFIG.cvars` / `CONFIG.secrets` existing.
pub fn build_config_map(plugin_name: &str) -> Map {
    let Some(ctx) = GLOBAL_CONTEXT.get() else {
        return build_config_map_from(plugin_name, &HashMap::new(), None, &HashMap::new());
    };

    let cvars = ctx
        .cvars
        .read()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    let caps = ctx
        .plugin_capabilities
        .read()
        .ok()
        .and_then(|m| m.get(plugin_name).cloned());
        
    let plugin_secrets = ctx
        .plugins
        .read()
        .ok()
        .and_then(|m| m.get(plugin_name).and_then(|c| c.secrets.clone()))
        .unwrap_or_default();

    build_config_map_from(plugin_name, &cvars, caps.as_ref(), &plugin_secrets)
}

/// Pure builder (no global state) used by [`build_config_map`] and unit tests.
///
/// `cvars` is the full registry; only entries whose `plugin_name` matches are
/// included. `caps` supplies the granted `env_allowlist` resolved against the
/// host environment (only when the `env` capability is granted).
pub fn build_config_map_from(
    plugin_name: &str,
    cvars: &HashMap<String, CVar>,
    caps: Option<&CapabilitySet>,
    plugin_secrets: &HashMap<String, String>,
) -> Map {
    let mut root = Map::new();
    root.insert("plugin".into(), Dynamic::from(plugin_name.to_string()));

    // Scoped cvars: only those owned by this plugin.
    let mut cvars_map = Map::new();
    for (name, cvar) in cvars.iter() {
        if cvar.plugin_name == plugin_name {
            cvars_map.insert(name.as_str().into(), Dynamic::from(cvar.value.clone()));
        }
    }
    root.insert("cvars".into(), Dynamic::from_map(cvars_map));

    // Granted secrets: resolved env vars from the plugin's allowlist.
    let mut secrets_map = Map::new();
    if let Some(caps) = caps
        && caps.has(Capability::Env)
    {
        for key in &caps.env_allowlist {
            if let Ok(val) = std::env::var(key) {
                secrets_map.insert(key.as_str().into(), Dynamic::from(val));
            }
        }
    }
    
    // Configured secrets in chaosnexus-anvil.toml
    for (key, val) in plugin_secrets {
        secrets_map.insert(key.as_str().into(), Dynamic::from(val.clone()));
    }
    
    root.insert("secrets".into(), Dynamic::from_map(secrets_map));

    root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cvar(plugin: &str, name: &str, value: &str) -> CVar {
        CVar {
            plugin_name: plugin.to_string(),
            name: name.to_string(),
            value: value.to_string(),
            description: String::new(),
        }
    }

    #[test]
    fn config_includes_only_owning_plugins_cvars() {
        let mut cvars = HashMap::new();
        cvars.insert("mine".to_string(), cvar("alpha", "mine", "1"));
        cvars.insert("theirs".to_string(), cvar("beta", "theirs", "2"));

        let config = build_config_map_from("alpha", &cvars, None, &HashMap::new());
        let cvars_map = config
            .get("cvars")
            .and_then(|d| d.read_lock::<Map>())
            .expect("cvars map present");

        assert!(cvars_map.contains_key("mine"));
        assert!(!cvars_map.contains_key("theirs"));
    }

    #[test]
    fn secrets_require_env_capability_and_allowlist() {
        // SAFETY: single-threaded test mutating a uniquely-named env var.
        unsafe {
            std::env::set_var("CHAOS_CONFIG_TEST_TOKEN", "s3cr3t");
        }

        let mut granted = CapabilitySet::from_id_list(&["env".to_string()]);
        granted.env_allowlist = vec!["CHAOS_CONFIG_TEST_TOKEN".to_string()];
        let config = build_config_map_from("alpha", &HashMap::new(), Some(&granted), &HashMap::new());
        let secrets = config
            .get("secrets")
            .and_then(|d| d.read_lock::<Map>())
            .expect("secrets map present");
        assert_eq!(
            secrets
                .get("CHAOS_CONFIG_TEST_TOKEN")
                .and_then(|d| d.clone().into_string().ok())
                .as_deref(),
            Some("s3cr3t")
        );

        // Without the env capability, the same allowlist yields nothing.
        let denied = CapabilitySet::default();
        let config_denied = build_config_map_from("alpha", &HashMap::new(), Some(&denied), &HashMap::new());
        let secrets_denied = config_denied
            .get("secrets")
            .and_then(|d| d.read_lock::<Map>())
            .expect("secrets map present");
        assert!(secrets_denied.is_empty());

        unsafe {
            std::env::remove_var("CHAOS_CONFIG_TEST_TOKEN");
        }
    }
}
