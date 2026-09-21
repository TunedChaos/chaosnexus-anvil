// chaosnexus-anvil/src/scripting/scaffold.rs
//
// Single source of truth for scaffolding new Rhai plugin folders under
// `scripts/plugins/`. Used by the built-in `cn_a_create_plugin` MCP
// tool and by the privileged `sys_install_plugin` Rhai native.

use crate::scripting::paths::{disabled_plugins_root, pending_root, plugins_root};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Built-in MCP tool names that must not be claimed by plugin scripts.
pub const RESERVED_TOOL_NAMES: &[&str] =
    &["cn_a_reload_plugins", "cn_a_cvars", "cn_a_create_plugin", "cn_a_disable_plugin"];

/// Paths written when a plugin folder is scaffolded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldResult {
    pub plugin_dir: PathBuf,
    pub toml_path: PathBuf,
    pub script_path: PathBuf,
}

/// Filename for quarantine metadata inside a pending plugin folder.
pub const PENDING_MANIFEST: &str = "chaosnexus-forge.pending.toml";

/// Quarantine manifest written alongside staged plugin files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingManifest {
    pub plugin_name: String,
    pub tool_name: String,
    pub description: String,
    pub requested_capabilities: Vec<String>,
    pub created_at: String,
}

/// Options for LLM-driven quarantine scaffolding.
#[derive(Debug, Clone)]
pub struct PendingScaffoldOptions {
    pub plugin_name: String,
    pub tool_name: String,
    pub description: String,
    pub input_schema: Option<String>,
    pub script_body: Option<String>,
    pub custom_toml: Option<String>,
    pub requested_capabilities: Vec<String>,
    pub overwrite: bool,
}

/// Options for templated plugin scaffolding (`cn_a_create_plugin`).
#[derive(Debug, Clone)]
pub struct ScaffoldOptions {
    pub plugin_name: String,
    pub tool_name: String,
    pub description: String,
    /// JSON Schema string for the MCP tool input. Defaults to an empty object.
    pub input_schema: Option<String>,
    /// When set, written verbatim as the entry `.rhai` file (LLM-authored).
    pub script_body: Option<String>,
    /// When set, used as the custom plugin.toml, with name/author enforced.
    pub custom_toml: Option<String>,
    pub overwrite: bool,
}

/// Validates a plugin folder name for engine discovery and sandboxing.
pub fn validate_plugin_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Plugin name cannot be empty.".into());
    }
    if name == "disabled" {
        return Err("Plugin name 'disabled' is reserved.".into());
    }
    if name.contains('/') || name.contains('\\') {
        return Err("Plugin name must not contain path separators.".into());
    }
    let valid = regex::Regex::new(r"^[a-z0-9_]+$")
        .expect("plugin name regex is valid")
        .is_match(name);
    if !valid {
        return Err("Plugin name must match ^[a-z0-9_]+$.".into());
    }
    Ok(())
}

/// Validates an MCP tool name before registration or scaffolding.
pub fn validate_tool_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Tool name cannot be empty.".into());
    }
    if name.contains('/') || name.contains('\\') {
        return Err("Tool name must not contain path separators.".into());
    }
    if RESERVED_TOOL_NAMES.contains(&name) {
        return Err(format!("Tool name '{}' is reserved.", name));
    }
    Ok(())
}

/// Returns the canonical entry script file name for a plugin folder.
pub fn entry_script_name(plugin_name: &str) -> String {
    format!("{plugin_name}_tool.rhai")
}

/// Renders `plugin.toml` for a newly scaffolded plugin.
pub fn render_plugin_toml(plugin_name: &str, description: &str) -> String {
    let desc_escaped = description.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "name = \"{plugin_name}\"\nversion = \"0.1.0\"\nauthor = \"ChaosNexus Anvil\"\ndescription = \"{desc_escaped}\"\nprefix = \"{plugin_name}\"\ndependencies = []\n"
    )
}

/// Escapes a string for embedding inside a Rhai double-quoted literal.
fn escape_rhai_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Derives a valid Rhai function identifier from an MCP tool name.
fn sanitize_handler_fn_name(tool_name: &str) -> String {
    let mut out: String = tool_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        out = "handle_tool".to_string();
    } else if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out = format!("handle_{out}");
    }
    out
}

/// Default JSON Schema for a stub MCP tool with no required arguments.
pub fn default_input_schema() -> &'static str {
    r#"{"type":"object","properties":{},"required":[]}"#
}

/// Renders a complete entry Rhai script with `register_mcp_tool` + `execute`.
pub fn render_entry_script(
    plugin_name: &str,
    tool_name: &str,
    description: &str,
    input_schema: &str,
) -> String {
    let rhai_filename = entry_script_name(plugin_name);
    let handler_fn = sanitize_handler_fn_name(tool_name);
    let desc_escaped = escape_rhai_string(description);
    let schema_escaped = escape_rhai_string(input_schema);
    let mut tool_with_prefix = tool_name.to_string();
    let expected_prefix = format!("cn_a_{plugin_name}_");
    if !tool_with_prefix.starts_with(&expected_prefix) {
        tool_with_prefix = format!("{expected_prefix}{tool_name}");
    }
    let tool_escaped = escape_rhai_string(&tool_with_prefix);

    format!(
        r#"// {rhai_filename}
// Autogenerated by ChaosNexus Anvil
//
// Two immutable constants are injected into every function scope:
//   PLUGIN_NAME   - this plugin's name (pass as the first arg to natives).
//   CONFIG        - identity-scoped config: CONFIG.cvars.<name> for this
//                   plugin's cvars and CONFIG.secrets.<NAME> for granted env
//                   secrets. Prefer CONFIG over get_env for tokens/keys.

// --- [NODE: on_plugin_start] ---
fn on_plugin_start() {{
    let schema_str = "{schema_escaped}";
    register_mcp_tool(PLUGIN_NAME, "{tool_escaped}", "{desc_escaped}", schema_str);
}}

// --- [NODE: on_plugin_stop] ---
fn on_plugin_stop() {{
    log_info("Plugin {plugin_name} stopped.");
}}

// --- [NODE: {handler_fn}] ---
fn {handler_fn}(args) {{
    return "Plugin {plugin_name} tool {tool_name} executed.";
}}

// --- [NODE: execute] ---
fn execute(tool_name, args) {{
    if tool_name == "{tool_escaped}" {{
        return {handler_fn}(args);
    }}
    return "Unknown tool";
}}
"#
    )
}

/// Writes `plugin.toml` and the entry script under `plugins/<plugin_name>/`.
pub fn install_plugin_files(
    plugin_name: &str,
    script_name: &str,
    toml_content: &str,
    script_content: &str,
    overwrite: bool,
) -> Result<ScaffoldResult, String> {
    if plugin_name.contains('/') || plugin_name.contains('\\') || plugin_name == "disabled" {
        return Err("Invalid plugin name for installation.".into());
    }
    if script_name.contains('/') || script_name.contains('\\') {
        return Err("Invalid script name.".into());
    }

    let plugin_dir = plugins_root().join(plugin_name);
    if plugin_dir.exists() && !overwrite {
        return Err(format!(
            "Plugin folder '{}' already exists. Pass overwrite=true to replace it.",
            plugin_name
        ));
    }

    std::fs::create_dir_all(&plugin_dir)
        .map_err(|e| format!("Failed to create plugin directory: {e}"))?;

    let toml_path = plugin_dir.join("plugin.toml");
    let script_path = plugin_dir.join(script_name);

    std::fs::write(&toml_path, toml_content)
        .map_err(|e| format!("Failed to write plugin.toml: {e}"))?;
    std::fs::write(&script_path, script_content)
        .map_err(|e| format!("Failed to write entry script: {e}"))?;

    Ok(ScaffoldResult {
        plugin_dir,
        toml_path,
        script_path,
    })
}

/// Scaffolds a new plugin using the canonical template or a provided script body.
pub fn scaffold_plugin(opts: ScaffoldOptions) -> Result<ScaffoldResult, String> {
    validate_plugin_name(&opts.plugin_name)?;
    validate_tool_name(&opts.tool_name)?;

    let description = opts.description;

    let input_schema = opts
        .input_schema
        .clone()
        .unwrap_or_else(|| default_input_schema().to_string());

    serde_json::from_str::<serde_json::Value>(&input_schema)
        .map_err(|e| format!("Invalid input_schema JSON: {e}"))?;

    let script_content = if let Some(body) = opts.script_body {
        body
    } else {
        render_entry_script(
            &opts.plugin_name,
            &opts.tool_name,
            &description,
            &input_schema,
        )
    };

    let toml_content = if let Some(ref custom) = opts.custom_toml {
        let mut parsed: toml::Value = toml::from_str(custom).map_err(|e| format!("Invalid custom_toml: {}", e))?;
        if let Some(table) = parsed.as_table_mut() {
            table.insert("name".to_string(), toml::Value::String(opts.plugin_name.clone()));
            if !table.contains_key("version") {
                table.insert("version".to_string(), toml::Value::String("0.1.0".to_string()));
            }
            if !table.contains_key("author") {
                table.insert("author".to_string(), toml::Value::String("ChaosNexus Anvil".to_string()));
            }
            if !table.contains_key("description") {
                table.insert("description".to_string(), toml::Value::String(description.clone()));
            }
            if !table.contains_key("prefix") {
                table.insert("prefix".to_string(), toml::Value::String(opts.plugin_name.clone()));
            }
        }
        toml::to_string(&parsed).map_err(|e| format!("Failed to re-serialize custom_toml: {}", e))?
    } else {
        render_plugin_toml(&opts.plugin_name, &description)
    };
    let expected_script = entry_script_name(&opts.plugin_name);

    install_plugin_files(
        &opts.plugin_name,
        &expected_script,
        &toml_content,
        &script_content,
        opts.overwrite,
    )
}

/// Writes plugin files under `scripts/.pending/<name>/` (never loaded by the engine).
pub fn install_pending_files(
    plugin_name: &str,
    script_name: &str,
    toml_content: &str,
    script_content: &str,
    manifest: &PendingManifest,
    overwrite: bool,
) -> Result<ScaffoldResult, String> {
    if plugin_name.contains('/') || plugin_name.contains('\\') || plugin_name == "disabled" {
        return Err("Invalid plugin name for pending install.".into());
    }
    if script_name.contains('/') || script_name.contains('\\') {
        return Err("Invalid script name.".into());
    }

    let plugin_dir = pending_root().join(plugin_name);
    if plugin_dir.exists() && !overwrite {
        return Err(format!(
            "Pending plugin '{}' already exists. Pass overwrite=true to replace it.",
            plugin_name
        ));
    }

    std::fs::create_dir_all(&plugin_dir)
        .map_err(|e| format!("Failed to create pending directory: {e}"))?;

    let toml_path = plugin_dir.join("plugin.toml");
    let script_path = plugin_dir.join(script_name);
    let manifest_path = plugin_dir.join(PENDING_MANIFEST);

    std::fs::write(&toml_path, toml_content)
        .map_err(|e| format!("Failed to write plugin.toml: {e}"))?;
    std::fs::write(&script_path, script_content)
        .map_err(|e| format!("Failed to write entry script: {e}"))?;
    let manifest_toml = toml::to_string(manifest)
        .map_err(|e| format!("Failed to serialize pending manifest: {e}"))?;
    std::fs::write(&manifest_path, manifest_toml)
        .map_err(|e| format!("Failed to write pending manifest: {e}"))?;

    Ok(ScaffoldResult {
        plugin_dir,
        toml_path,
        script_path,
    })
}

/// Scaffolds a plugin into quarantine awaiting ChaosNexus Forge human approval.
pub fn scaffold_pending_plugin(opts: PendingScaffoldOptions) -> Result<ScaffoldResult, String> {
    validate_plugin_name(&opts.plugin_name)?;
    validate_tool_name(&opts.tool_name)?;

    let description = opts.description;

    let input_schema = opts
        .input_schema
        .clone()
        .unwrap_or_else(|| default_input_schema().to_string());

    serde_json::from_str::<serde_json::Value>(&input_schema)
        .map_err(|e| format!("Invalid input_schema JSON: {e}"))?;

    let script_content = if let Some(body) = opts.script_body {
        body
    } else {
        render_entry_script(
            &opts.plugin_name,
            &opts.tool_name,
            &description,
            &input_schema,
        )
    };

    let toml_content = if let Some(ref custom) = opts.custom_toml {
        let mut parsed: toml::Value = toml::from_str(custom).map_err(|e| format!("Invalid custom_toml: {}", e))?;
        if let Some(table) = parsed.as_table_mut() {
            table.insert("name".to_string(), toml::Value::String(opts.plugin_name.clone()));
            if !table.contains_key("version") {
                table.insert("version".to_string(), toml::Value::String("0.1.0".to_string()));
            }
            if !table.contains_key("author") {
                table.insert("author".to_string(), toml::Value::String("ChaosNexus Anvil".to_string()));
            }
            if !table.contains_key("description") {
                table.insert("description".to_string(), toml::Value::String(description.clone()));
            }
            if !table.contains_key("prefix") {
                table.insert("prefix".to_string(), toml::Value::String(opts.plugin_name.clone()));
            }
        }
        toml::to_string(&parsed).map_err(|e| format!("Failed to re-serialize custom_toml: {}", e))?
    } else {
        render_plugin_toml(&opts.plugin_name, &description)
    };
    let expected_script = entry_script_name(&opts.plugin_name);

    let manifest = PendingManifest {
        plugin_name: opts.plugin_name.clone(),
        tool_name: opts.tool_name.clone(),
        description: description.clone(),
        requested_capabilities: opts.requested_capabilities.clone(),
        created_at: Utc::now().to_rfc3339(),
    };

    install_pending_files(
        &opts.plugin_name,
        &expected_script,
        &toml_content,
        &script_content,
        &manifest,
        opts.overwrite,
    )
}

/// Promotes a pending plugin folder into `plugins/` with granted capabilities.
pub fn promote_pending_plugin(
    plugin_name: &str,
    granted_capabilities: &[String],
    env_allowlist: &[String],
) -> Result<ScaffoldResult, String> {
    validate_plugin_name(plugin_name)?;

    let pending_dir = pending_root().join(plugin_name);
    if !pending_dir.is_dir() {
        return Err(format!("Pending plugin '{}' not found.", plugin_name));
    }

    let live_dir = plugins_root().join(plugin_name);
    if live_dir.exists() {
        return Err(format!(
            "Live plugin '{}' already exists. Remove it before promoting.",
            plugin_name
        ));
    }

    let script_name = entry_script_name(plugin_name);
    let pending_script = pending_dir.join(&script_name);
    let pending_toml = pending_dir.join("plugin.toml");
    if !pending_script.is_file() || !pending_toml.is_file() {
        return Err(format!(
            "Pending plugin '{}' is missing required files.",
            plugin_name
        ));
    }

    std::fs::rename(&pending_dir, &live_dir)
        .map_err(|e| format!("Failed to promote pending plugin: {e}"))?;

    let mut caps =
        crate::scripting::capabilities::CapabilitySet::from_id_list(granted_capabilities);
    caps.env_allowlist = env_allowlist.to_vec();

    let caps_section = caps.render_toml_section();
    let mut toml_content = std::fs::read_to_string(live_dir.join("plugin.toml"))
        .map_err(|e| format!("Failed to read promoted plugin.toml: {e}"))?;
    if !toml_content.contains("[capabilities]") {
        if !toml_content.ends_with('\n') {
            toml_content.push('\n');
        }
        toml_content.push_str(&caps_section);
        std::fs::write(live_dir.join("plugin.toml"), &toml_content)
            .map_err(|e| format!("Failed to write capabilities: {e}"))?;
    }

    let _ = std::fs::remove_file(live_dir.join(PENDING_MANIFEST));

    Ok(ScaffoldResult {
        plugin_dir: live_dir.clone(),
        toml_path: live_dir.join("plugin.toml"),
        script_path: live_dir.join(script_name),
    })
}

/// Rejects (deletes) a pending plugin folder.
pub fn reject_pending_plugin(plugin_name: &str) -> Result<(), String> {
    validate_plugin_name(plugin_name)?;
    let pending_dir = pending_root().join(plugin_name);
    if !pending_dir.is_dir() {
        return Err(format!("Pending plugin '{}' not found.", plugin_name));
    }
    std::fs::remove_dir_all(&pending_dir)
        .map_err(|e| format!("Failed to reject pending plugin: {e}"))
}

/// Moves a live plugin folder into `plugins/disabled/<name>/` so discovery skips it.
pub fn disable_plugin(plugin_name: &str) -> Result<PathBuf, String> {
    validate_plugin_name(plugin_name)?;

    let source = plugins_root().join(plugin_name);
    let dest = disabled_plugins_root().join(plugin_name);

    if !source.is_dir() {
        if dest.is_dir() {
            return Err(format!("Plugin '{plugin_name}' is already disabled."));
        }
        return Err(format!("Plugin '{plugin_name}' not found under plugins/."));
    }

    if dest.exists() {
        return Err(format!(
            "Cannot disable '{plugin_name}': plugins/disabled/{plugin_name} already exists."
        ));
    }

    std::fs::create_dir_all(disabled_plugins_root())
        .map_err(|e| format!("Failed to create plugins/disabled: {e}"))?;

    std::fs::rename(&source, &dest).map_err(|e| format!("Failed to disable plugin: {e}"))?;
    Ok(dest)
}

/// Relative path for a disabled plugin (user-facing).
pub fn relative_disabled_path(plugin_name: &str) -> String {
    format!(
        "plugins/disabled/{plugin_name}/{}",
        entry_script_name(plugin_name)
    )
}

/// Relative path for a pending plugin (user-facing).
pub fn relative_pending_path(plugin_name: &str) -> String {
    format!(".pending/{plugin_name}/{}", entry_script_name(plugin_name))
}

/// Returns true when `path` is an existing plugin directory under `plugins/`.
pub fn plugin_dir_exists(plugin_name: &str) -> bool {
    plugins_root().join(plugin_name).is_dir()
}

/// Relative display path from the scripts root for user-facing messages.
pub fn relative_plugin_path(result: &ScaffoldResult) -> String {
    format!(
        "plugins/{}/{}",
        result
            .plugin_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(""),
        result
            .script_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_plugin_name_rejects_reserved_and_invalid() {
        assert!(validate_plugin_name("my_plugin").is_ok());
        assert!(validate_plugin_name("disabled").is_err());
        assert!(validate_plugin_name("Bad-Name").is_err());
        assert!(validate_plugin_name("../escape").is_err());
    }

    #[test]
    fn render_entry_script_includes_register_and_execute() {
        let script = render_entry_script(
            "git_log",
            "git_log_june",
            "List commits",
            default_input_schema(),
        );
        assert!(script.contains("register_mcp_tool"));
        assert!(script.contains("fn execute(tool_name, args)"));
        assert!(script.contains("git_log_june"));
    }
}
