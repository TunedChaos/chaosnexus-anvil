// chaosnexus-anvil/src/scripting/schema.rs
//
// Single Source of Truth (SSOT) schema builder.
//
// Rhai's `gen_fn_metadata_to_json` emits a flat `{ "functions": [...] }`
// document. ChaosNexus Forge's Monaco providers and the VitePress documentation
// generator both consume the richer `modules[]` contract described in
// `chaosnexus-full.md`. This module transforms the raw Rhai metadata into
// that contract so the engine remains the single source of truth for every
// downstream documentation and autocomplete surface.
//
// Output shape:
// {
//   "meta":    { "version": "<crate>", "generated_at": "<rfc3339>" },
//   "modules": {
//     "<module>": {
//       "description": "...",
//       "functions": [
//         {
//           "name": "...",
//           "signature": "...",
//           "parameters":  [ { "name", "type", "description" } ],
//           "return_type": "...",
//           "description": "...",
//           "docs_url": "https://chaosnexus.ai/api/rhai/<module>/<name>"
//         }
//       ]
//     }
//   }
// }

use serde_json::{Map, Value, json};

/// Public documentation base URL. Hover links and generated VitePress pages
/// share this prefix so a docs link in the IDE always resolves to a real page.
const DOCS_BASE_URL: &str = "https://chaosnexus.ai/api/rhai";

/// Human-readable blurb for the implicit global namespace. Rhai registers all
/// ChaosNexus Anvil natives globally, so virtually every function lands here.
const GLOBAL_MODULE_DESCRIPTION: &str = "Global ChaosNexus Anvil engine functions available to every Rhai plugin without an explicit import.";

/// Transforms raw Rhai engine metadata into the documented `modules[]` contract.
///
/// `raw_metadata` is the JSON produced by `Engine::gen_fn_metadata_to_json`.
/// Returns a pretty-printed JSON string. On parse failure the raw metadata is
/// returned unchanged so the caller always receives valid JSON.
pub fn transform_metadata(raw_metadata: &str) -> String {
    let parsed: Value = match serde_json::from_str(raw_metadata) {
        Ok(value) => value,
        // Return-early: if Rhai ever changes its format, degrade gracefully
        // rather than panicking the schema pipeline.
        Err(_) => return raw_metadata.to_string(),
    };

    let empty: Vec<Value> = Vec::new();
    let functions = parsed
        .get("functions")
        .and_then(Value::as_array)
        .unwrap_or(&empty);

    // module name -> ordered list of function objects (dedup by name).
    let mut modules: Map<String, Value> = Map::new();

    for func in functions {
        let Some(name) = func.get("name").and_then(Value::as_str) else {
            continue;
        };

        // Skip private, internal, and operator/non-identifier entries; these
        // are noise for both autocomplete and documentation.
        if !is_documentable(func, name) {
            continue;
        }

        let module_name = module_for(func);
        let docs_url = format!("{}/{}/{}", DOCS_BASE_URL, module_name, name);
        let (description, param_docs) = parse_doc_comments(func);
        let parameters = build_parameters(func, &param_docs);

        let entry = json!({
            "name": name,
            "signature": func.get("signature").and_then(Value::as_str).unwrap_or(name),
            "parameters": parameters,
            "return_type": func
                .get("returnType")
                .and_then(Value::as_str)
                .unwrap_or("()"),
            "description": description,
            "docs_url": docs_url,
        });

        let module = modules.entry(module_name.clone()).or_insert_with(
            || json!({ "description": module_description(&module_name), "functions": [] }),
        );

        if let Some(list) = module.get_mut("functions").and_then(Value::as_array_mut) {
            // Dedup overloads by name; prefer the variant carrying doc comments.
            if let Some(existing) = list
                .iter_mut()
                .find(|f| f.get("name").and_then(Value::as_str) == Some(name))
            {
                if existing
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .is_empty()
                {
                    *existing = entry;
                }
            } else {
                list.push(entry);
            }
        }
    }

    let output = json!({
        "meta": {
            "version": env!("CARGO_PKG_VERSION"),
            "generated_at": chrono::Utc::now().to_rfc3339(),
        },
        "modules": modules,
    });

    serde_json::to_string_pretty(&output).unwrap_or_else(|_| raw_metadata.to_string())
}

/// Returns true when a function should appear in docs and autocomplete.
fn is_documentable(func: &Value, name: &str) -> bool {
    // Return-early on private access.
    if func.get("access").and_then(Value::as_str) == Some("private") {
        return false;
    }
    // Return-early on the internal namespace.
    if func.get("namespace").and_then(Value::as_str) == Some("internal") {
        return false;
    }
    is_identifier(name)
}

/// Determines the logical documentation module for a function. Rhai registers
/// ChaosNexus Anvil natives in the global namespace, so this currently collapses to
/// `global`; the structure leaves room for future static modules (e.g. `mcp`).
fn module_for(func: &Value) -> String {
    match func.get("namespace").and_then(Value::as_str) {
        Some("global") | None => "global".to_string(),
        Some(other) => other.to_string(),
    }
}

/// Provides a description blurb for a module name.
fn module_description(module_name: &str) -> String {
    match module_name {
        "global" => GLOBAL_MODULE_DESCRIPTION.to_string(),
        other => format!("ChaosNexus Anvil `{}` module functions.", other),
    }
}

/// True if `name` is a valid Rhai identifier (filters operators like `+`, `==`).
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Builds the parameter list, merging type info from the engine signature with
/// any descriptions extracted from the function's doc comments.
fn build_parameters(func: &Value, param_docs: &[(String, String)]) -> Value {
    let mut params: Vec<Value> = Vec::new();

    if let Some(raw_params) = func.get("params").and_then(Value::as_array) {
        for (index, param) in raw_params.iter().enumerate() {
            let name = param
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("arg{}", index));
            let ty = param
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("Dynamic")
                .to_string();

            // Match a documented description by parameter name.
            let description = param_docs
                .iter()
                .find(|(doc_name, _)| doc_name == &name)
                .map(|(_, desc)| desc.clone())
                .unwrap_or_default();

            params.push(json!({
                "name": name,
                "type": ty,
                "description": description,
            }));
        }
    }

    Value::Array(params)
}

/// Parses `///` doc comments into a clean description plus a list of
/// `(param_name, description)` pairs harvested from an `### Arguments` block.
///
/// Recognized argument bullet forms (after comment-marker stripping):
/// * `` * `name` - description ``
/// * `` * `name`: description ``
fn parse_doc_comments(func: &Value) -> (String, Vec<(String, String)>) {
    let Some(comments) = func.get("docComments").and_then(Value::as_array) else {
        return (String::new(), Vec::new());
    };

    // Rhai may emit each doc comment as a single multi-line string (a whole
    // `///` block) rather than one array element per line. Flatten both shapes
    // into individual lines before parsing.
    let lines: Vec<String> = comments
        .iter()
        .filter_map(Value::as_str)
        .flat_map(|block| block.split('\n'))
        .map(str::to_string)
        .collect();

    let mut description_lines: Vec<String> = Vec::new();
    let mut param_docs: Vec<(String, String)> = Vec::new();
    let mut in_arguments = false;
    // Once the first description paragraph ends we stop collecting summary text,
    // but keep scanning so an `### Arguments` block further down is still parsed.
    let mut description_done = false;

    for raw in &lines {
        let cleaned = clean_comment_line(raw);
        let trimmed = cleaned.trim();

        if let Some(heading) = trimmed.strip_prefix('#') {
            // A markdown heading ends the free-form description. Track whether
            // we are entering the arguments section.
            in_arguments = heading.to_ascii_lowercase().contains("argument");
            description_done = true;
            continue;
        }

        if in_arguments {
            if let Some((name, desc)) = parse_argument_bullet(trimmed) {
                param_docs.push((name, desc));
            }
            continue;
        }

        // Collect only the first description paragraph for a concise summary.
        if description_done {
            continue;
        }
        if !trimmed.is_empty() {
            description_lines.push(trimmed.to_string());
        } else if !description_lines.is_empty() {
            description_done = true;
        }
    }

    (description_lines.join(" "), param_docs)
}

/// Strips Rust doc-comment markers (`///`, `/**`, `*`, `*/`) from a line.
fn clean_comment_line(line: &str) -> String {
    let trimmed = line.trim_start();
    let stripped = trimmed
        .strip_prefix("///")
        .or_else(|| trimmed.strip_prefix("/**"))
        .or_else(|| trimmed.strip_prefix("*/"))
        .or_else(|| trimmed.strip_prefix("//!"))
        .or_else(|| trimmed.strip_prefix('*'))
        .unwrap_or(trimmed);
    stripped.trim().to_string()
}

/// Parses a single `### Arguments` bullet into `(name, description)`.
fn parse_argument_bullet(line: &str) -> Option<(String, String)> {
    // Expect a bullet marker.
    let body = line.strip_prefix('*').or_else(|| line.strip_prefix('-'))?;
    let body = body.trim();

    // Parameter name is wrapped in backticks.
    let body = body.strip_prefix('`')?;
    let close = body.find('`')?;
    let name = body[..close].trim().to_string();
    if name.is_empty() {
        return None;
    }

    let rest = body[close + 1..].trim();
    let desc = rest
        .strip_prefix('-')
        .or_else(|| rest.strip_prefix(':'))
        .unwrap_or(rest)
        .trim()
        .to_string();

    Some((name, desc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn transforms_flat_functions_into_modules() {
        let raw = r#"{
            "functions": [
                {
                    "namespace": "global",
                    "access": "public",
                    "name": "register_mcp_tool",
                    "signature": "register_mcp_tool(plugin_name: &str, tool_name: &str) -> ()",
                    "returnType": "()",
                    "params": [
                        { "name": "plugin_name", "type": "&str" },
                        { "name": "tool_name", "type": "&str" }
                    ],
                    "docComments": [
                        "/// Registers a new tool on this MCP server.",
                        "///",
                        "/// ### Arguments",
                        "/// * `plugin_name` - The internal name of the plugin.",
                        "/// * `tool_name` - The unique tool name."
                    ]
                }
            ]
        }"#;

        let out = transform_metadata(raw);
        let parsed: Value = serde_json::from_str(&out).unwrap();

        assert!(parsed.get("meta").is_some());
        let func = &parsed["modules"]["global"]["functions"][0];
        assert_eq!(func["name"], "register_mcp_tool");
        assert_eq!(
            func["description"],
            "Registers a new tool on this MCP server."
        );
        assert_eq!(
            func["docs_url"],
            "https://chaosnexus.ai/api/rhai/global/register_mcp_tool"
        );
        assert_eq!(
            func["parameters"][0]["description"],
            "The internal name of the plugin."
        );
        assert_eq!(func["parameters"][1]["name"], "tool_name");
    }

    #[test]
    fn parses_single_block_doc_comments() {
        // Rhai often emits one multi-line string per function rather than one
        // line per array element.
        let raw = r#"{
            "functions": [
                {
                    "namespace": "global",
                    "access": "public",
                    "name": "log_info",
                    "signature": "log_info(plugin_name: &str, message: &str) -> ()",
                    "returnType": "()",
                    "params": [
                        { "name": "plugin_name", "type": "&str" },
                        { "name": "message", "type": "&str" }
                    ],
                    "docComments": ["/// Writes an info-level log line.\n///\n/// ### Arguments\n/// * `plugin_name` - Owning plugin.\n/// * `message` - The text to log."]
                }
            ]
        }"#;

        let out = transform_metadata(raw);
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let func = &parsed["modules"]["global"]["functions"][0];
        assert_eq!(func["description"], "Writes an info-level log line.");
        assert_eq!(func["parameters"][0]["description"], "Owning plugin.");
        assert_eq!(func["parameters"][1]["description"], "The text to log.");
    }

    #[test]
    fn skips_operators_and_private_functions() {
        let raw = r#"{
            "functions": [
                { "namespace": "global", "access": "public", "name": "+", "signature": "+" },
                { "namespace": "global", "access": "private", "name": "secret_helper", "signature": "secret_helper()" },
                { "namespace": "internal", "access": "public", "name": "hidden", "signature": "hidden()" }
            ]
        }"#;

        let out = transform_metadata(raw);
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let modules = parsed["modules"].as_object().unwrap();

        // No documentable functions remain, so no modules are emitted.
        assert!(modules.is_empty());
    }

    #[test]
    fn returns_valid_json_on_garbage_input() {
        let out = transform_metadata("not json");
        assert_eq!(out, "not json");
    }
}
