// chaosnexus-anvil/src/scripting/secrets.rs
//
// Capability-gated secret broker replacing ambient `get_env` for sensitive values.

use crate::scripting::capabilities::Capability;
use crate::scripting::plugin_context::{PluginCapabilities, capabilities_for, current_plugin};

use crate::scripting::models::NativeContext;

/// Reads an environment variable only when the caller has `env` + allowlist entry.
/// Or reads directly from the [plugins.plugin_name.secrets] block in config.
pub fn get_secret(ctx: &NativeContext, key: &str) -> Result<String, String> {
    let plugin = current_plugin().unwrap_or_else(|| "unknown".to_string());
    
    // First, check the plugin's secrets config
    if let Ok(plugins) = ctx.plugins.read()
        && let Some(config) = plugins.get(&plugin)
        && let Some(secrets) = &config.secrets
        && let Some(val) = secrets.get(key)
    {
        return Ok(val.clone());
    }

    // Fall back to environment variables if allowed by capabilities
    let caps = capabilities_for(&ctx.plugin_capabilities, &plugin);
    if !caps.env_var_allowed(key) {
        return Err(format!(
            "Environment variable '{key}' is not in the granted env_allowlist for plugin '{plugin}', nor is it in the plugin's secrets block."
        ));
    }
    std::env::var(key).map_err(|e| format!("get_env error for '{key}': {e}"))
}

/// Registers a check used by native `get_env` gate.
pub fn require_env_capability(registry: &PluginCapabilities) -> Result<(), String> {
    let plugin = current_plugin().unwrap_or_else(|| "unknown".to_string());
    let caps = capabilities_for(registry, &plugin);
    caps.require(Capability::Env)
}
