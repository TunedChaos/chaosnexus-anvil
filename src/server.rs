use crate::scripting::capabilities::parse_requested_capabilities;
use crate::scripting::manager::PluginManager;
use crate::scripting::scaffold::{self, PendingScaffoldOptions};
use rust_mcp_sdk::McpServer;
use rust_mcp_sdk::McpClient;
use rust_mcp_sdk::mcp_server::ServerHandler;
use rust_mcp_sdk::schema::schema_utils::CallToolError;
use rust_mcp_sdk::schema::{CallToolRequestParams, CallToolResult, ListToolsResult, Tool};
use serde_json::{Map, Value};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Convenience macro to define an MCP [`Tool`] with a name, description, and JSON input schema.
macro_rules! define_tool {
    ($name:expr, $desc:expr, $schema:tt) => {
        Tool {
            name: $name.to_string(),
            description: Some($desc.to_string()),
            input_schema: serde_json::from_value(serde_json::json!($schema)).unwrap(),
            meta: None,
            output_schema: None,
            title: None,
            annotations: None,
            execution: None,
            icons: Vec::new(),
        }
    };
}


/// Returns the tool schema for the built-in `cn_a_reload_plugins` tool.
fn reload_plugins_tool_schema() -> Tool {
    define_tool!(
        "cn_a_reload_plugins",
        "Reloads all plugins natively, gracefully dropping the engine and rebuilding without restarting the server.",
        {
            "type": "object",
            "properties": {}
        }
    )
}

/// Returns the tool schema for the built-in `cn_a_check_plugin_status` tool.
fn check_plugin_status_tool_schema() -> Tool {
    define_tool!(
        "cn_a_check_plugin_status",
        "Check if a plugin is currently pending user approval, approved and active, or missing/rejected. Use this to poll for status updates after creating a plugin.",
        {
            "type": "object",
            "properties": {
                "plugin_name": {
                    "type": "string",
                    "description": "The name of the plugin to check."
                }
            },
            "required": ["plugin_name"]
        }
    )
}

/// Returns the tool schema for the built-in `cn_a_cvars` tool.
fn cvars_tool_schema() -> Tool {
    define_tool!(
        "cn_a_cvars",
        "Lists or modifies runtime CVars for all plugins. If both name and value are provided, updates the CVar and triggers on_cvar_changed. If omitted, lists all CVars and their descriptions.",
        {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "(Optional) The name of the CVar to update."
                },
                "value": {
                    "type": "string",
                    "description": "(Optional) The new value of the CVar."
                }
            }
        }
    )
}

fn disable_plugin_tool_schema() -> Tool {
    define_tool!(
        "cn_a_disable_plugin",
        "Disables a live plugin by moving its folder to plugins/disabled/ (never loaded). Reloads all plugins when complete.",
        {
            "type": "object",
            "properties": {
                "plugin_name": {
                    "type": "string",
                    "description": "Folder name under scripts/plugins/ (^[a-z0-9_]+$)."
                }
            },
            "required": ["plugin_name"]
        }
    )
}

fn get_ide_connection_info_tool_schema() -> Tool {
    define_tool!(
        "cn_a_get_ide_connection_info",
        "Retrieves the SSE port and secure token required for the IDE to connect to this ChaosNexus Anvil instance. Use this when the user needs to initiate a manual remote connection to an active supervised engine.",
        {
            "type": "object",
            "properties": {}
        }
    )
}

fn get_status_tool_schema() -> Tool {
    define_tool!(
        "cn_a_get_status",
        "Returns the active read-only configuration settings and the absolute paths to script directories. Use this to orient the agent to the environment.",
        {
            "type": "object",
            "properties": {}
        }
    )
}

fn get_plugin_config_tool_schema() -> Tool {
    define_tool!(
        "cn_a_get_plugin_config",
        "Returns the effective runtime configuration for a specific plugin, including its parsed plugin.toml metadata, active runtime CVars, and security capabilities.",
        {
            "type": "object",
            "properties": {
                "plugin_name": {
                    "type": "string",
                    "description": "The name of the plugin folder (e.g. 'check_server')."
                }
            },
            "required": ["plugin_name"]
        }
    )
}

fn create_plugin_tool_schema() -> Tool {
    define_tool!(
        "cn_a_create_plugin",
        "Scaffolds a new Rhai plugin into quarantine (scripts/.pending/). The plugin is NOT loaded until a human approves it in ChaosNexus Forge. CRITICAL: The script body MUST include comprehensive code comments explaining exactly what the plugin does so non-technical users can review and trust the code before approving it.",
        {
            "type": "object",
            "properties": {
                "plugin_name": {
                    "type": "string",
                    "description": "Folder name (^[a-z0-9_]+$). Entry script becomes <plugin_name>_tool.rhai."
                },
                "tool_name": {
                    "type": "string",
                    "description": "Globally unique MCP tool name the script will register after approval."
                },
                "description": {
                    "type": "string",
                    "description": "Human-readable description for plugin.toml and register_mcp_tool. MUST explain exactly what the tool does so other LLMs can use it."
                },
                "input_schema": {
                    "type": "string",
                    "description": "JSON Schema string for the MCP tool input. Defaults to an empty object schema."
                },
                "script_body": {
                    "type": "string",
                    "description": "Optional full Rhai entry script. CRITICAL: MUST include comprehensive code comments explaining exactly what the script does so non-technical users can review and trust it. When provided, written verbatim instead of the stub template."
                },
                "requested_capabilities": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Capabilities the script requests (human may grant a subset on approve). See security_model.md."
                },
                "overwrite": {
                    "type": "boolean",
                    "description": "When true, replace an existing pending folder. Defaults to false."
                }
            },
            "required": ["plugin_name", "tool_name", "description"]
        }
    )
}

fn search_plugins_tool_schema() -> Tool {
    define_tool!(
        "cn_a_search_plugins",
        "Searches the official ChaosNexus Anvil plugin registry for available plugins.",
        {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "(Optional) Search term to filter plugins by name or description."
                }
            }
        }
    )
}

fn install_plugin_tool_schema() -> Tool {
    define_tool!(
        "cn_a_install_plugin",
        "Downloads and stages a plugin from a Git repository into the local .pending/ quarantine directory. The plugin is NOT active until the human user approves it in the ChaosNexus Forge IDE.",
        {
            "type": "object",
            "properties": {
                "git_url": {
                    "type": "string",
                    "description": "A direct Git repository URL to install the plugin from."
                },
                "plugin_name": {
                    "type": "string",
                    "description": "The name of the plugin folder to extract from the repository."
                }
            },
            "required": ["git_url", "plugin_name"]
        }
    )
}

fn read_plugin_tool_schema() -> Tool {
    define_tool!(
        "cn_a_read_plugin",
        "Reads the raw source code and metadata (plugin.toml) of a currently installed plugin. Use this to understand how an existing plugin works or before making modifications.",
        {
            "type": "object",
            "properties": {
                "plugin_name": {
                    "type": "string",
                    "description": "The directory name of the plugin to read (e.g. 'http_client')."
                }
            },
            "required": ["plugin_name"]
        }
    )
}

fn arg_str(args: &Option<Map<String, Value>>, key: &str) -> Option<String> {
    args.as_ref()?.get(key)?.as_str().map(str::to_string)
}

fn arg_bool(args: &Option<Map<String, Value>>, key: &str, default: bool) -> bool {
    args.as_ref()
        .and_then(|map| map.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

async fn handle_read_plugin(
    plugin_manager: &Arc<RwLock<PluginManager>>,
    args: &Option<Map<String, Value>>,
) -> Result<CallToolResult, CallToolError> {
    let plugin_name = arg_str(args, "plugin_name")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CallToolError::from_message("plugin_name is required"))?;

    scaffold::validate_plugin_name(&plugin_name).map_err(CallToolError::from_message)?;

    let pm = plugin_manager.read().await;
    let plugin_path = pm.scripts_root().join("plugins").join(&plugin_name);
    
    if !plugin_path.exists() || !plugin_path.is_dir() {
        return Err(CallToolError::from_message(format!("Plugin '{}' not found in {}", plugin_name, pm.scripts_root().join("plugins").display())));
    }

    let toml_path = plugin_path.join("plugin.toml");
    let script_path = plugin_path.join(format!("{plugin_name}_tool.rhai"));

    let metadata = if toml_path.exists() {
        std::fs::read_to_string(&toml_path).unwrap_or_else(|_| "Error reading plugin.toml".to_string())
    } else {
        "plugin.toml not found".to_string()
    };

    let script = if script_path.exists() {
        std::fs::read_to_string(&script_path).unwrap_or_else(|_| "Error reading script file".to_string())
    } else {
        "Script file not found".to_string()
    };

    Ok(CallToolResult::text_content(vec![
        rust_mcp_sdk::schema::TextContent::from(format!(
            "--- plugin.toml ---\n{}\n\n--- {}_tool.rhai ---\n{}",
            metadata, plugin_name, script
        )),
    ]))
}

async fn handle_create_plugin(
    plugin_manager: &Arc<RwLock<PluginManager>>,
    args: &Option<Map<String, Value>>,
) -> Result<CallToolResult, CallToolError> {
    let plugin_name = arg_str(args, "plugin_name")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CallToolError::from_message("plugin_name is required"))?;
    let tool_name = arg_str(args, "tool_name")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CallToolError::from_message("tool_name is required"))?;

    scaffold::validate_plugin_name(&plugin_name).map_err(CallToolError::from_message)?;
    scaffold::validate_tool_name(&tool_name).map_err(CallToolError::from_message)?;

    {
        let pm = plugin_manager.read().await;
        if pm.tool_exists(&tool_name) {
            return Err(CallToolError::from_message(format!(
                "Tool name '{tool_name}' is already registered. Choose a different tool_name."
            )));
        }
    }

    let overwrite = arg_bool(args, "overwrite", false);
    let description = arg_str(args, "description")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            CallToolError::from_message(
                "description is required and must explain exactly what the tool does",
            )
        })?;
    let input_schema = arg_str(args, "input_schema");
    let script_body = arg_str(args, "script_body");
    let custom_toml = arg_str(args, "custom_toml");
    let requested_capabilities = args
        .as_ref()
        .and_then(|m| m.get("requested_capabilities"))
        .map(parse_requested_capabilities)
        .unwrap_or_default();

    if let Some(script) = &script_body
        && !script.contains("//")
        && !script.contains("/*")
    {
        return Err(CallToolError::from_message(
            "CRITICAL ERROR: script_body MUST contain comments explaining what the plugin does for human review. Tool scaffolding rejected.",
        ));
    }

    scaffold::scaffold_pending_plugin(PendingScaffoldOptions {
        plugin_name: plugin_name.clone(),
        tool_name: tool_name.clone(),
        description,
        input_schema,
        script_body: script_body.clone(),
        custom_toml,
        requested_capabilities: requested_capabilities.clone(),
        overwrite,
    })
    .map_err(CallToolError::from_message)?;

    let rel_path = scaffold::relative_pending_path(&plugin_name);

    let mut msg = format!(
        "PENDING human approval in ChaosNexus Forge.\n\n\
         Staged quarantined plugin at {rel_path} (tool: {tool_name}).\n\
         Scaffolded files:\n\
         - plugin.toml\n\
         - {}_tool.rhai\n\n\
         The plugin was NOT loaded. It will automatically load and notify MCP clients once approved by the user.\n\n\
         INSTRUCTION FOR LLM: Tell the user exactly what to do next. Do not wait for them. If the plugin requires configuration in chaosnexus-anvil.toml (such as network or database permissions), provide the exact TOML snippet to append to the end of their chaosnexus-anvil.toml file.
         ",
        plugin_name
    );

    let has_network = requested_capabilities.iter().any(|c| c.contains("net_"));
    let has_db = requested_capabilities.iter().any(|c| c.contains("db_"));

    if has_network || has_db {
        msg.push_str("\n\nREQUIRED CONFIGURATION:\nInstruct the user to add the following block to their `chaosnexus-anvil.toml` file located in the root of their workspace:\n\n```toml\n");
        msg.push_str(&format!("[plugins.{}.permissions]\n", plugin_name));
        
        if has_network {
            msg.push_str("http_domains = [\"localhost\"]\n");
            msg.push_str("http = [\"GET\", \"POST\"]\n");
        }
        if has_db {
            msg.push_str("sql_urls = [\"sqlite://\"]\n");
            msg.push_str("sql = [\"SELECT\"]\n");
        }
        
        msg.push_str("```\n");
        msg.push_str("Remind the user to modify the above domains and operations to precisely match the external endpoints this plugin needs to access.\n");
    }

    let has_fs_req = requested_capabilities
        .iter()
        .any(|c| c == "fs_cross_plugin")
        || script_body.as_ref().is_some_and(|s| s.contains("fs_"));

    if has_fs_req {
        let plugin_path = std::fs::canonicalize(crate::scripting::paths::plugins_root())
            .unwrap_or_else(|_| crate::scripting::paths::plugins_root())
            .join(&plugin_name);
        let shared_data_path = std::fs::canonicalize(crate::scripting::paths::data_root())
            .unwrap_or_else(|_| crate::scripting::paths::data_root());

        msg.push_str(&format!(
            "\n\nIMPORTANT FS CAPABILITY INSTRUCTIONS:\n\
            If your plugin needs to store or read its own internal files, you MUST place them in your plugin's directory:\n\
            {plugin_path}\n\
            \n\
            If you need to interact with data that is shared across multiple plugins, you MUST use the shared data directory:\n\
            {shared_data_path}\n\
            \n\
            You must inform the user about where to place any files they wish to provide to this plugin.",
            plugin_path = plugin_path.display(),
            shared_data_path = shared_data_path.display()
        ));
    }

    Ok(CallToolResult::text_content(vec![
        rust_mcp_sdk::schema::TextContent::from(msg),
    ]))
}

async fn handle_disable_plugin(
    plugin_manager: &Arc<RwLock<PluginManager>>,
    args: &Option<Map<String, Value>>,
) -> Result<CallToolResult, CallToolError> {
    let plugin_name = arg_str(args, "plugin_name")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CallToolError::from_message("plugin_name is required"))?;

    scaffold::validate_plugin_name(&plugin_name).map_err(CallToolError::from_message)?;

    let dest = scaffold::disable_plugin(&plugin_name).map_err(CallToolError::from_message)?;
    let rel = scaffold::relative_disabled_path(&plugin_name);

    let mut pm = plugin_manager.write().await;
    pm.rebuild_from_disk();

    Ok(CallToolResult::text_content(vec![
        rust_mcp_sdk::schema::TextContent::from(format!(
            "Plugin '{plugin_name}' disabled at {rel} (on disk: {}). To permanently remove it, manually delete that folder.",
            dest.display()
        )),
    ]))
}

async fn handle_search_plugins(
    args: &Option<Map<String, Value>>,
) -> Result<CallToolResult, CallToolError> {
    let query = arg_str(args, "query").unwrap_or_default().to_lowercase();
    
    // Temporary mock registry until we publish the official chaos-plugins git repo.
    let mock_registry = vec![
        serde_json::json!({
            "name": "pkg_manager",
            "description": "Installs Node/Python packages for external tasks.",
            "git_url": "https://git.tnd.cx/Tuned_Chaos/chaos-plugins.git",
            "capabilities": ["shell", "process_spawn"]
        }),
        serde_json::json!({
            "name": "docker_manager",
            "description": "Manages local docker containers for testing.",
            "git_url": "https://git.tnd.cx/Tuned_Chaos/chaos-plugins.git",
            "capabilities": ["shell"]
        }),
    ];

    let filtered: Vec<_> = mock_registry.into_iter().filter(|p| {
        if query.is_empty() { return true; }
        let name = p["name"].as_str().unwrap_or("").to_lowercase();
        let desc = p["description"].as_str().unwrap_or("").to_lowercase();
        name.contains(&query) || desc.contains(&query)
    }).collect();

    let json_str = serde_json::to_string_pretty(&filtered).unwrap_or_default();
    Ok(CallToolResult::text_content(vec![
        rust_mcp_sdk::schema::TextContent::from(json_str),
    ]))
}

async fn handle_install_plugin(
    args: &Option<Map<String, Value>>,
) -> Result<CallToolResult, CallToolError> {
    let plugin_name = arg_str(args, "plugin_name")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CallToolError::from_message("plugin_name is required"))?;
    let git_url = arg_str(args, "git_url")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CallToolError::from_message("git_url is required"))?;

    scaffold::validate_plugin_name(&plugin_name).map_err(CallToolError::from_message)?;

    let temp_dir = std::env::temp_dir().join(format!("cn_a_clone_{}", uuid::Uuid::new_v4()));
    
    let clone_status = tokio::process::Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg(&git_url)
        .arg(&temp_dir)
        .status()
        .await
        .map_err(|e| CallToolError::from_message(format!("Failed to execute git clone: {}", e)))?;

    if !clone_status.success() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(CallToolError::from_message(format!("Git clone failed for URL: {}", git_url)));
    }

    let source_plugin_dir = temp_dir.join(&plugin_name);
    if !source_plugin_dir.exists() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(CallToolError::from_message(format!("Plugin folder '{}' not found in cloned repository.", plugin_name)));
    }

    let pending_dir = crate::scripting::paths::pending_root().join(&plugin_name);
    if pending_dir.exists() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(CallToolError::from_message(format!("Plugin '{}' is already in pending/ quarantine.", plugin_name)));
    }

    // Move the folder to pending
    std::fs::create_dir_all(crate::scripting::paths::pending_root())
        .map_err(|e| CallToolError::from_message(format!("Failed to create pending root: {}", e)))?;
        
    // Standard fs::rename can fail across mount points, so we copy then remove.
    // However, rust fs doesn't have a built in copy_dir. We'll use fs_extra or manual.
    // For simplicity since it's local temp to local workspace, we can try rename first.
    if let Err(e) = std::fs::rename(&source_plugin_dir, &pending_dir) {
        // Fallback to recursive copy
        let copy_status = tokio::process::Command::new("cp")
            .arg("-r")
            .arg(&source_plugin_dir)
            .arg(&pending_dir)
            .status()
            .await;
            
        if copy_status.is_err() || !copy_status.unwrap().success() {
            let _ = std::fs::remove_dir_all(&temp_dir);
            return Err(CallToolError::from_message(format!("Failed to move plugin to pending: {}", e)));
        }
    }
    
    // Ensure the pending manifest exists
    let toml_path = pending_dir.join("plugin.toml");
    let manifest_path = pending_dir.join(crate::scripting::scaffold::PENDING_MANIFEST);
    
    let description = if toml_path.exists() {
        let content = std::fs::read_to_string(&toml_path).unwrap_or_default();
        let value: toml::Value = toml::from_str(&content).unwrap_or(toml::Value::Table(toml::map::Map::new()));
        value.get("description").and_then(|v| v.as_str()).unwrap_or("Installed via Git").to_string()
    } else {
        "Installed via Git".to_string()
    };
    
    let tool_name = format!("cn_a_{}_tool", plugin_name);

    if !manifest_path.exists() {
        let manifest = crate::scripting::scaffold::PendingManifest {
            plugin_name: plugin_name.clone(),
            tool_name: tool_name.clone(),
            description,
            requested_capabilities: vec![],
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        let manifest_toml = toml::to_string(&manifest).unwrap_or_default();
        let _ = std::fs::write(&manifest_path, manifest_toml);
    }

    let _ = std::fs::remove_dir_all(&temp_dir);

    Ok(CallToolResult::text_content(vec![
        rust_mcp_sdk::schema::TextContent::from(format!(
            "Successfully downloaded '{}' to quarantine (.pending/). The plugin will remain inactive until a human audits and approves it in the ChaosNexus Forge UI.",
            plugin_name
        )),
    ]))
}



/// The primary MCP server handler for ChaosNexus Anvil.
///
/// Routes incoming MCP `call_tool` and `list_tools` requests to plugin scripts,
/// built-in tools, or proxied remote MCP connections.
#[derive(Clone)]
pub struct Handler {
    /// The shared plugin manager holding all loaded Rhai plugin ASTs.
    pub plugin_manager: Arc<RwLock<PluginManager>>,
    /// Map of connection IDs to remote MCP client runtimes for proxy forwarding.
    pub proxy_clients: Arc<RwLock<std::collections::HashMap<String, Arc<rust_mcp_sdk::mcp_client::ClientRuntime>>>>,
    /// Maximum allowed response length from proxied MCP calls.
    pub max_proxy_response_length: usize,
}

#[async_trait::async_trait]
impl ServerHandler for Handler {
    async fn handle_list_tools_request(
        &self,
        _params: Option<rust_mcp_sdk::schema::PaginatedRequestParams>,
        _runtime: Arc<dyn McpServer>,
    ) -> Result<ListToolsResult, rust_mcp_sdk::schema::RpcError> {
        let mut tools: Vec<Tool> = Vec::new();

        let pm = self.plugin_manager.read().await;
        pm.register_tools(&mut tools);
        drop(pm);

        let proxy_map = self.proxy_clients.read().await;
        for (prefix, client) in proxy_map.iter() {
            if let Ok(list) = client.request_tool_list(None).await {
                for mut tool in list.tools {
                    let stripped = tool.name.strip_prefix(&format!("{prefix}_")).unwrap_or(&tool.name);
                    tool.name = format!("cn_a__{prefix}_{}", stripped);
                    tools.push(tool);
                }
            }
        }
        drop(proxy_map);

        // Built-in reload tool
        tools.push(reload_plugins_tool_schema());
        tools.push(check_plugin_status_tool_schema());
        tools.push(create_plugin_tool_schema());
        tools.push(disable_plugin_tool_schema());
        tools.push(search_plugins_tool_schema());
        tools.push(install_plugin_tool_schema());
        tools.push(read_plugin_tool_schema());

        tools.push(get_ide_connection_info_tool_schema());
        tools.push(get_status_tool_schema());
        tools.push(get_plugin_config_tool_schema());
        // Built-in CVars tool
        tools.push(cvars_tool_schema());

        Ok(ListToolsResult {
            tools,
            meta: None,
            next_cursor: None,
        })
    }

    async fn handle_call_tool_request(
        &self,
        params: CallToolRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> Result<CallToolResult, CallToolError> {
        use crate::scripting::trace::{RECURSION_LIMIT_ERROR, hop_from_meta};

        let hop = hop_from_meta(&params.meta);
        if hop > crate::scripting::trace::MAX_HOP_COUNT {
            return Err(CallToolError::from_message(RECURSION_LIMIT_ERROR));
        }

        match params.name.as_str() {
            "cn_a_get_ide_connection_info" => {
                if let Some(ctx) = crate::scripting::models::GLOBAL_CONTEXT.get()
                    && let Some((port, token)) = &ctx.ide_connection_info
                {
                    return Ok(CallToolResult::text_content(vec![
                        rust_mcp_sdk::schema::TextContent::from(format!(
                            "{{\n  \"host\": \"127.0.0.1\",\n  \"port\": {},\n  \"token\": \"{}\"\n}}",
                            port, token
                        )),
                    ]));
                }
                return Err(CallToolError::from_message(
                    "IDE connection info is not available. The SSE server may not be running."
                        .to_string(),
                ));
            }
            "cn_a_get_status" => {
                let pm = self.plugin_manager.read().await;
                let scripts_root = pm.scripts_root().to_path_buf();
                let config_json = serde_json::to_value(&pm.config)
                    .map_err(|e| CallToolError::from_message(format!("Failed to serialize config: {}", e)))?;
                
                return Ok(CallToolResult::text_content(vec![
                    rust_mcp_sdk::schema::TextContent::from(format!(
                        "{{\n  \"scripts_root\": \"{}\",\n  \"config\": {}\n}}",
                        scripts_root.display(),
                        serde_json::to_string_pretty(&config_json).unwrap_or_default()
                    )),
                ]));
            }
            "cn_a_get_plugin_config" => {
                let plugin_name = arg_str(&params.arguments, "plugin_name")
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| CallToolError::from_message("plugin_name is required"))?;

                let pm = self.plugin_manager.read().await;
                let plugin_path = pm.scripts_root().join("plugins").join(&plugin_name);
                let toml_path = plugin_path.join("plugin.toml");

                let raw_metadata = if toml_path.exists() {
                    let content = std::fs::read_to_string(&toml_path).unwrap_or_default();
                    let parsed: serde_json::Value = toml::from_str(&content).unwrap_or(serde_json::json!({}));
                    parsed
                } else {
                    serde_json::json!({"error": "plugin.toml not found"})
                };

                let overrides = pm.config.plugins.as_ref().and_then(|m| m.get(&plugin_name)).cloned();
                let overrides_json = serde_json::to_value(&overrides).unwrap_or(serde_json::json!(null));

                return Ok(CallToolResult::text_content(vec![
                    rust_mcp_sdk::schema::TextContent::from(format!(
                        "{{\n  \"plugin_name\": \"{}\",\n  \"raw_metadata\": {},\n  \"security_overrides\": {}\n}}",
                        plugin_name,
                        serde_json::to_string_pretty(&raw_metadata).unwrap_or_default(),
                        serde_json::to_string_pretty(&overrides_json).unwrap_or_default()
                    )),
                ]));
            }
            "cn_a_check_plugin_status" => {
                let plugin_name = arg_str(&params.arguments, "plugin_name")
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| CallToolError::from_message("plugin_name is required"))?;

                let pm = self.plugin_manager.read().await;
                let scripts_root = pm.scripts_root();
                let pending_path = scripts_root.join(".pending").join(&plugin_name);
                let active_path = scripts_root.join("plugins").join(&plugin_name);

                let status = if active_path.exists() {
                    format!("Plugin '{}' is APPROVED and ACTIVE in the plugins directory.", plugin_name)
                } else if pending_path.exists() {
                    format!("Plugin '{}' is PENDING user approval.", plugin_name)
                } else {
                    format!("Plugin '{}' does NOT exist (it may have been rejected or deleted).", plugin_name)
                };

                return Ok(CallToolResult::text_content(vec![
                    rust_mcp_sdk::schema::TextContent::from(status),
                ]));
            }
            "cn_a_reload_plugins" => {
                let mut pm = self.plugin_manager.write().await;
                pm.rebuild_from_disk();
                return Ok(CallToolResult::text_content(vec![
                    rust_mcp_sdk::schema::TextContent::from(
                        "Successfully stopped all plugins and rebuilt engine from disk.".to_string(),
                    ),
                ]));
            }
            "cn_a_create_plugin" => {
                return handle_create_plugin(&self.plugin_manager, &params.arguments).await;
            }
            "cn_a_disable_plugin" => {
                return handle_disable_plugin(&self.plugin_manager, &params.arguments).await;
            }
            "cn_a_search_plugins" => {
                return handle_search_plugins(&params.arguments).await;
            }
            "cn_a_install_plugin" => {
                return handle_install_plugin(&params.arguments).await;
            }
            "cn_a_read_plugin" => {
                return handle_read_plugin(&self.plugin_manager, &params.arguments).await;
            }
            "cn_a_cvars" => {
                let pm = self.plugin_manager.read().await;
                let name = arg_str(&params.arguments, "name");
                let value = arg_str(&params.arguments, "value");

                if let (Some(n), Some(v)) = (name, value) {
                    let msg = pm.set_cvar(&n, &v).map_err(CallToolError::from_message)?;
                    return Ok(CallToolResult::text_content(vec![
                        rust_mcp_sdk::schema::TextContent::from(msg),
                    ]));
                } else {
                    let out = pm.list_cvars();
                    return Ok(CallToolResult::text_content(vec![
                        rust_mcp_sdk::schema::TextContent::from(out),
                    ]));
                }
            }
            _ => {}
        }

        let pm = self.plugin_manager.read().await;
        if let Some(res) = pm.handle_tool(&params.name, &params).await? {
            return Ok(res);
        }
        drop(pm);

        let proxy_map = self.proxy_clients.read().await;
        for (prefix, client) in proxy_map.iter() {
            let prefix_marker = format!("cn_a__{prefix}_");
            if params.name.starts_with(&prefix_marker) {
                let actual_tool_name = params.name.strip_prefix(&prefix_marker).unwrap();
                let mut proxied_params = params.clone();
                proxied_params.name = format!("{prefix}_{actual_tool_name}");

                let mut res = client.request_tool_call(proxied_params).await.map_err(|e| CallToolError::from_message(e.to_string()))?;
                truncate_call_tool_result(&mut res, self.max_proxy_response_length);
                return Ok(res);
            }
        }

        Err(CallToolError::unknown_tool(params.name))
    }

    async fn handle_list_resources_request(
        &self,
        _params: Option<rust_mcp_sdk::schema::PaginatedRequestParams>,
        _runtime: Arc<dyn McpServer>,
    ) -> Result<rust_mcp_sdk::schema::ListResourcesResult, rust_mcp_sdk::schema::RpcError> {
        let mut resources: Vec<rust_mcp_sdk::schema::Resource> = Vec::new();
        let pm = self.plugin_manager.read().await;
        pm.register_resources(&mut resources);
        drop(pm);

        let proxy_map = self.proxy_clients.read().await;
        for (prefix, client) in proxy_map.iter() {
            if let Ok(list) = client.request_resource_list(None).await {
                for mut resource in list.resources {
                    let stripped_uri = resource.uri.strip_prefix(&format!("{prefix}_")).unwrap_or(&resource.uri);
                    let stripped_name = resource.name.strip_prefix(&format!("{prefix}_")).unwrap_or(&resource.name);
                    resource.uri = format!("cn_a__{prefix}_{}", stripped_uri);
                    resource.name = format!("cn_a__{prefix}_{}", stripped_name);
                    resources.push(resource);
                }
            }
        }



        Ok(rust_mcp_sdk::schema::ListResourcesResult {
            resources,
            meta: None,
            next_cursor: None,
        })
    }

    async fn handle_read_resource_request(
        &self,
        params: rust_mcp_sdk::schema::ReadResourceRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> Result<rust_mcp_sdk::schema::ReadResourceResult, rust_mcp_sdk::schema::RpcError> {


        let pm = self.plugin_manager.read().await;
        if let Ok(Some(output)) = pm.handle_resource(&params.uri).await {
            return Ok(rust_mcp_sdk::schema::ReadResourceResult {
                contents: vec![
                    rust_mcp_sdk::schema::ReadResourceContent::TextResourceContents(
                        rust_mcp_sdk::schema::TextResourceContents {
                            uri: params.uri.clone(),
                            mime_type: None,
                            text: output,
                            meta: None,
                        },
                    ),
                ],
                meta: None,
            });
        }
        drop(pm);

        let proxy_map = self.proxy_clients.read().await;
        for (prefix, client) in proxy_map.iter() {
            let prefix_marker = format!("cn_a__{prefix}_");
            if params.uri.starts_with(&prefix_marker) {
                let actual_uri = params.uri.strip_prefix(&prefix_marker).unwrap();
                let mut proxied_params = params.clone();
                proxied_params.uri = format!("{prefix}_{actual_uri}");

                let mut res = client.request_resource_read(proxied_params).await.map_err(|e| {
                    rust_mcp_sdk::schema::RpcError::new(
                        rust_mcp_sdk::schema::RpcErrorCodes::INTERNAL_ERROR,
                        e.to_string(),
                        None,
                    )
                })?;
                truncate_read_resource_result(&mut res, self.max_proxy_response_length);
                return Ok(res);
            }
        }

        Err(rust_mcp_sdk::schema::RpcError::new(
            rust_mcp_sdk::schema::RpcErrorCodes::INVALID_REQUEST,
            format!("Resource not found: {}", params.uri),
            None,
        ))
    }

    async fn handle_list_prompts_request(
        &self,
        _params: Option<rust_mcp_sdk::schema::PaginatedRequestParams>,
        _runtime: Arc<dyn McpServer>,
    ) -> Result<rust_mcp_sdk::schema::ListPromptsResult, rust_mcp_sdk::schema::RpcError> {
        let mut prompts: Vec<rust_mcp_sdk::schema::Prompt> = Vec::new();
        let pm = self.plugin_manager.read().await;
        pm.register_prompts(&mut prompts);
        drop(pm);

        let proxy_map = self.proxy_clients.read().await;
        for (prefix, client) in proxy_map.iter() {
            if let Ok(list) = client.request_prompt_list(None).await {
                for mut prompt in list.prompts {
                    let stripped_name = prompt.name.strip_prefix(&format!("{prefix}_")).unwrap_or(&prompt.name);
                    prompt.name = format!("cn_a__{prefix}_{}", stripped_name);
                    prompts.push(prompt);
                }
            }
        }
        Ok(rust_mcp_sdk::schema::ListPromptsResult {
            prompts,
            meta: None,
            next_cursor: None,
        })
    }

    async fn handle_get_prompt_request(
        &self,
        params: rust_mcp_sdk::schema::GetPromptRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> Result<rust_mcp_sdk::schema::GetPromptResult, rust_mcp_sdk::schema::RpcError> {
        let pm = self.plugin_manager.read().await;
        if let Ok(Some(output)) = pm.handle_prompt(&params.name, &params).await {
            return Ok(output);
        }
        drop(pm);

        let proxy_map = self.proxy_clients.read().await;
        for (prefix, client) in proxy_map.iter() {
            let prefix_marker = format!("cn_a__{prefix}_");
            if params.name.starts_with(&prefix_marker) {
                let actual_name = params.name.strip_prefix(&prefix_marker).unwrap();
                let mut proxied_params = params.clone();
                proxied_params.name = format!("{prefix}_{actual_name}");

                let mut res = client.request_prompt(proxied_params).await.map_err(|e| {
                    rust_mcp_sdk::schema::RpcError::new(
                        rust_mcp_sdk::schema::RpcErrorCodes::INTERNAL_ERROR,
                        e.to_string(),
                        None,
                    )
                })?;
                truncate_get_prompt_result(&mut res, self.max_proxy_response_length);
                return Ok(res);
            }
        }

        Err(rust_mcp_sdk::schema::RpcError::new(
            rust_mcp_sdk::schema::RpcErrorCodes::INVALID_REQUEST,
            format!("Prompt not found: {}", params.name),
            None,
        ))
    }
}

fn truncate_call_tool_result(res: &mut rust_mcp_sdk::schema::CallToolResult, max_len: usize) {
    let mut total_len = 0;
    for content in res.content.iter_mut() {
        if let rust_mcp_sdk::schema::ContentBlock::TextContent(t) = content {
            total_len += t.text.len();
            if total_len > max_len {
                let overflow = total_len - max_len;
                let allowed = t.text.len().saturating_sub(overflow);
                if allowed < t.text.len() {
                    let mut new_text = t.text.chars().take(allowed).collect::<String>();
                    new_text.push_str(&format!("\n\n[TRUNCATED: Response exceeded ChaosNexus Anvil proxy limit of {} bytes]", max_len));
                    t.text = new_text;
                }
            }
        }
    }
}

fn truncate_read_resource_result(res: &mut rust_mcp_sdk::schema::ReadResourceResult, max_len: usize) {
    let mut total_len = 0;
    for content in res.contents.iter_mut() {
        if let rust_mcp_sdk::schema::ReadResourceContent::TextResourceContents(t) = content {
            total_len += t.text.len();
            if total_len > max_len {
                let overflow = total_len - max_len;
                let allowed = t.text.len().saturating_sub(overflow);
                if allowed < t.text.len() {
                    let mut new_text = t.text.chars().take(allowed).collect::<String>();
                    new_text.push_str(&format!("\n\n[TRUNCATED: Response exceeded ChaosNexus Anvil proxy limit of {} bytes]", max_len));
                    t.text = new_text;
                }
            }
        }
    }
}

fn truncate_get_prompt_result(res: &mut rust_mcp_sdk::schema::GetPromptResult, max_len: usize) {
    let mut total_len = 0;
    let msg = &mut res.messages;
    for m in msg {
            if let rust_mcp_sdk::schema::ContentBlock::TextContent(t) = &mut m.content {
                total_len += t.text.len();
                if total_len > max_len {
                    let overflow = total_len - max_len;
                    let allowed = t.text.len().saturating_sub(overflow);
                    if allowed < t.text.len() {
                        let mut new_text = t.text.chars().take(allowed).collect::<String>();
                        new_text.push_str(&format!("\n\n[TRUNCATED: Response exceeded ChaosNexus Anvil proxy limit of {} bytes]", max_len));
                        t.text = new_text;
                    }
                }
            }
}
}
