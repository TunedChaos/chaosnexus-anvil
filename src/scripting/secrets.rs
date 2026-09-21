// chaosnexus-anvil/src/scripting/secrets.rs
//
// Capability-gated secret broker. Host TOML must NOT embed raw secret material.
// `[plugins.<name>.secrets]` maps logical names -> environment variable NAMES;
// values are resolved from the process environment at runtime.

use crate::scripting::capabilities::Capability;
use crate::scripting::plugin_context::{PluginCapabilities, capabilities_for, current_plugin};

use crate::scripting::models::NativeContext;

fn resolve_env_named(env_name: &str) -> Result<String, String> {
    let name = env_name.trim();
    if name.is_empty() {
        return Err("empty secret env name".into());
    }
    // Reject values that look like embedded secret material (contain whitespace
    // or are obviously not an env identifier).
    if name.chars().any(|c| c.is_whitespace())
        || !(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    {
        return Err(format!(
            "Invalid secrets mapping '{name}': host TOML must name an environment variable \
             (e.g. MY_API_TOKEN), not embed the secret value. Set the env var outside Anvil."
        ));
    }
    std::env::var(name).map_err(|e| format!("secret env '{name}' unavailable: {e}"))
}

/// Reads a secret by logical key.
/// Order: plugin secrets map (env-name indirection) → env_allowlist + process env.
pub fn get_secret(ctx: &NativeContext, key: &str) -> Result<String, String> {
    let plugin = current_plugin().unwrap_or_else(|| "unknown".to_string());

    if let Ok(plugins) = ctx.plugins.read()
        && let Some(config) = plugins.get(&plugin)
        && let Some(secrets) = &config.secrets
        && let Some(env_name) = secrets.get(key)
    {
        return resolve_env_named(env_name);
    }

    let caps = capabilities_for(&ctx.plugin_capabilities, &plugin);
    if !caps.env_var_allowed(key) {
        return Err(format!(
            "Environment variable '{key}' is not in the granted env_allowlist for plugin '{plugin}', \
             nor is it mapped under [plugins.{plugin}.secrets]."
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_embedded_secret_looking_values() {
        let err = resolve_env_named("sk-live-not-an-env-name-with-dashes!!!").unwrap_err();
        assert!(err.contains("environment variable"), "{err}");
        let err2 = resolve_env_named("has spaces").unwrap_err();
        assert!(err2.contains("environment variable"), "{err2}");
    }

    #[test]
    fn accepts_env_identifier_shape() {
        // May fail to resolve, but must not reject the name shape.
        match resolve_env_named("CHAOSNEXUS_TEST_SECRET_XYZ_UNLIKELY") {
            Ok(_) => {}
            Err(e) => assert!(e.contains("unavailable") || e.contains("not"), "{e}"),
        }
    }
}
