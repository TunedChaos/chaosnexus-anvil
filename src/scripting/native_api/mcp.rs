// chaosnexus-anvil/src/scripting/native_api/mcp.rs
use crate::scripting::models::{NativeContext, global_context};
use rhai::Engine;
use rhai::plugin::*;
use rust_mcp_sdk::schema::{Prompt, PromptArgument, Resource, Tool, ToolInputSchema};

/// Verifies the identity of an MCP caller plugin.
fn verify_mcp_caller_identity(plugin_name: &str) -> Result<(), Box<rhai::EvalAltResult>> {
    let Some(ctx) = global_context() else {
        return Ok(());
    };
    let Some(current) = crate::scripting::plugin_context::current_plugin() else {
        return Ok(());
    };
    let caller_caps =
        crate::scripting::plugin_context::capabilities_for(&ctx.plugin_capabilities, &current);
    crate::scripting::plugin_context::verify_plugin_identity(
        plugin_name,
        &caller_caps,
        crate::scripting::capabilities::Capability::FsCrossPlugin,
    )
    .map_err(|e: String| -> Box<rhai::EvalAltResult> { e.into() })
}

/// Enforces the expected prefix on an MCP resource or tool name.
fn enforce_prefix(plugin_name: &str, requested_name: &str, ctx: &NativeContext) -> String {
    let prefix = ctx
        .plugin_prefixes
        .read()
        .unwrap()
        .get(plugin_name)
        .cloned()
        .unwrap_or_else(|| plugin_name.to_string());
    let expected_prefix = format!("cn_a_{prefix}_");
    if requested_name.starts_with(&expected_prefix) {
        requested_name.to_string()
    } else {
        let new_name = format!("{expected_prefix}{requested_name}");
        eprintln!(
            "[warn] Plugin '{plugin_name}' registered item '{requested_name}' without the expected prefix. Automatically prepended to '{new_name}'."
        );
        new_name
    }
}

#[export_module]
pub mod mcp_api {
    /// Registers a new tool on this MCP server.
    ///
    /// ### Arguments
    /// * `plugin_name` - The internal name of the plugin registering the tool.
    /// * `tool_name` - The unique name of the tool exposed to clients.
    /// * `desc` - A human-readable description of what the tool does.
    /// * `schema_json` - A JSON string representing the JSON Schema for the tool's input arguments.
    #[rhai_fn(return_raw)]
    pub fn register_mcp_tool(
        tool_name: &str,
        desc: &str,
        schema_json: &str,
    ) -> Result<rhai::Dynamic, Box<rhai::EvalAltResult>> {
        let plugin_name_owned = crate::scripting::plugin_context::current_plugin_name();
        let plugin_name = plugin_name_owned.as_str();
        if let Some(ctx) = global_context() {
            verify_mcp_caller_identity(plugin_name)?;
            let tool_name = enforce_prefix(plugin_name, tool_name, &ctx);
            let mut t_map = ctx.tools.lock().unwrap();
            if t_map.contains_key(&tool_name) {
                return Err(format!("Collision: Tool '{}' already registered", tool_name).into());
            }
            if desc.trim().len() < 10 {
                return Err(
                    "Tool description must be at least 10 characters explaining what it does."
                        .into(),
                );
            }
            let input_schema = serde_json::from_str::<ToolInputSchema>(schema_json)
                .map_err(|e| format!("Invalid schema JSON: {}", e))?;

            t_map.insert(
                tool_name.clone(),
                Tool {
                    name: tool_name.clone(),
                    description: Some(desc.to_string()),
                    input_schema,
                    meta: None,
                    output_schema: None,
                    title: None,
                    annotations: None,
                    execution: None,
                    icons: vec![],
                },
            );
            ctx.tool_owners
                .lock()
                .unwrap()
                .insert(tool_name, plugin_name.to_string());
        }
        Ok(rhai::Dynamic::UNIT)
    }

    /// Registers a new resource on this MCP server.
    ///
    /// ### Arguments
    /// * `plugin_name` - The internal name of the plugin registering the resource.
    /// * `resource_name` - The human-readable name of the resource.
    /// * `uri` - The unique URI identifying the resource (e.g., `file:///path`).
    /// * `desc` - A description of the resource's contents.
    #[rhai_fn(return_raw)]
    pub fn register_mcp_resource(
        resource_name: &str,
        uri: &str,
        desc: &str,
    ) -> Result<rhai::Dynamic, Box<rhai::EvalAltResult>> {
        let plugin_name_owned = crate::scripting::plugin_context::current_plugin_name();
        let plugin_name = plugin_name_owned.as_str();
        if let Some(ctx) = global_context() {
            verify_mcp_caller_identity(plugin_name)?;
            let resource_name = enforce_prefix(plugin_name, resource_name, &ctx);
            let uri = enforce_prefix(plugin_name, uri, &ctx);
            let mut r_map = ctx.resources.lock().unwrap();
            if r_map.contains_key(&uri) {
                return Err(format!("Collision: Resource URI '{}' already registered", uri).into());
            }
            r_map.insert(
                uri.clone(),
                Resource {
                    uri: uri.clone(),
                    name: resource_name,
                    description: Some(desc.to_string()),
                    mime_type: None,
                    annotations: None,
                    size: None,
                    meta: None,
                    icons: vec![],
                    title: None,
                },
            );
            ctx.resource_owners
                .lock()
                .unwrap()
                .insert(uri, plugin_name.to_string());
        }
        Ok(rhai::Dynamic::UNIT)
    }

    /// Registers a new prompt template on this MCP server.
    ///
    /// ### Arguments
    /// * `plugin_name` - The internal name of the plugin registering the prompt.
    /// * `prompt_name` - The unique name of the prompt template.
    /// * `desc` - A description of what the prompt provides.
    /// * `args_json` - A JSON string representing an array of PromptArgument objects (or an empty array/string).
    #[rhai_fn(return_raw)]
    pub fn register_mcp_prompt(
        prompt_name: &str,
        desc: &str,
        args_json: &str,
    ) -> Result<rhai::Dynamic, Box<rhai::EvalAltResult>> {
        let plugin_name_owned = crate::scripting::plugin_context::current_plugin_name();
        let plugin_name = plugin_name_owned.as_str();
        if let Some(ctx) = global_context() {
            verify_mcp_caller_identity(plugin_name)?;
            let prompt_name = enforce_prefix(plugin_name, prompt_name, &ctx);
            let mut p_map = ctx.prompts.lock().unwrap();
            if p_map.contains_key(&prompt_name) {
                return Err(
                    format!("Collision: Prompt '{}' already registered", prompt_name).into(),
                );
            }
            let arguments = if args_json.is_empty() {
                None
            } else {
                Some(
                    serde_json::from_str::<Vec<PromptArgument>>(args_json)
                        .map_err(|e| format!("Invalid prompt args JSON: {}", e))?,
                )
            };

            p_map.insert(
                prompt_name.clone(),
                Prompt {
                    name: prompt_name.clone(),
                    description: Some(desc.to_string()),
                    arguments: arguments.unwrap_or_default(),
                    meta: None,
                    icons: vec![],
                    title: None,
                },
            );
            ctx.prompt_owners
                .lock()
                .unwrap()
                .insert(prompt_name, plugin_name.to_string());
        }
        Ok(rhai::Dynamic::UNIT)
    }
}

/// Registers the `mcp_api` module with the given engine.
pub fn register(engine: &mut Engine, _n_ctx: &NativeContext) {
    let mcp_module = rhai::exported_module!(mcp_api);
    engine.register_global_module(mcp_module.into());

    // Overload 4-argument signatures (plugin_name, tool_name, desc, schema_json) for backward compatibility with scripts
    engine.register_fn(
        "register_mcp_tool",
        |_plugin_name: &str, tool_name: &str, desc: &str, schema_json: &str| -> Result<rhai::Dynamic, Box<rhai::EvalAltResult>> {
            mcp_api::register_mcp_tool(tool_name, desc, schema_json)
        },
    );

    engine.register_fn(
        "register_mcp_resource",
        |_plugin_name: &str, resource_name: &str, uri: &str, desc: &str| -> Result<rhai::Dynamic, Box<rhai::EvalAltResult>> {
            mcp_api::register_mcp_resource(resource_name, uri, desc)
        },
    );

    engine.register_fn(
        "register_mcp_prompt",
        |_plugin_name: &str, prompt_name: &str, desc: &str, args_json: &str| -> Result<rhai::Dynamic, Box<rhai::EvalAltResult>> {
            mcp_api::register_mcp_prompt(prompt_name, desc, args_json)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhai::Engine;

    #[test]
    fn test_register_mcp_tool_overloads() {
        let mut engine = Engine::new();
        let ctx = crate::scripting::engine::empty_context();
        register(&mut engine, &ctx);

        let script = r#"
            let schema = "{\"type\":\"object\",\"properties\":{}}";
            register_mcp_tool("tool_3_args", "A valid tool description of 10+ chars", schema);
            register_mcp_tool("my_plugin", "tool_4_args", "Another valid tool description", schema);
        "#;
        let res = engine.eval::<()>(script);
        assert!(res.is_ok(), "Failed to eval script with overloaded register_mcp_tool: {:?}", res.err());
    }
}

