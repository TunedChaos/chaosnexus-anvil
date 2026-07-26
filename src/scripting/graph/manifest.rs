// chaosnexus-anvil/src/scripting/graph/manifest.rs
//
// Function-signature extraction for the "Assembly Line" visual scripting model
// (Phase 6a). A plugin's `.rhai` file is a library of decoupled functions (the
// Actuators); the paired `.canvas.json` sidecar binds those functions into a
// data-flow topology. ChaosNexus Forge needs the authoritative list of functions and
// their parameters to render node handles and flag stale bindings.
//
// We extract signatures from the Rhai AST rather than via brittle frontend
// regexes: the engine compiles the script (microseconds, no execution) and
// walks `AST::iter_functions()`. Because Rhai resolves function calls at
// runtime, a bare `Engine` compiles a plugin that references native functions
// without any registration, so signature extraction stays dependency-free.

use serde::Serialize;

/// Visibility of a script function, mirrored from Rhai's `FnAccess`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FnAccess {
    /// Callable from outside the script (the default for `fn name() {}`).
    Public,
    /// Declared `private fn name() {}`; internal helper only.
    Private,
}

/// A single extracted function signature. Each public function becomes a
/// candidate node on the canvas; `params` become its inbound data handles and
/// the return payload its single outbound handle.
#[derive(Debug, Clone, Serialize)]
pub struct FnSignature {
    /// Function name (the node's bound identifier).
    pub name: String,
    /// Ordered parameter names; map 1:1 to inbound wire handles.
    pub params: Vec<String>,
    /// Declared visibility.
    pub access: FnAccess,
    /// Leading `///` / `//!` doc comments, joined with newlines (may be empty).
    pub doc: String,
}

/// Extracts every function signature from Rhai `source`.
///
/// Returns a compile error string on syntax errors so callers can surface it as
/// a node error flag. Functions are returned in source order.
pub fn extract_function_signatures(source: &str) -> Result<Vec<FnSignature>, String> {
    // A bare engine is sufficient: compilation only parses syntax and never
    // executes `import` statements or resolves native calls.
    let engine = rhai::Engine::new();
    let ast = engine.compile(source).map_err(|e| e.to_string())?;

    let signatures = ast
        .iter_functions()
        .map(|f| FnSignature {
            name: f.name.to_string(),
            params: f.params.iter().map(|p| p.to_string()).collect(),
            // `rhai::FnAccess` is `#[non_exhaustive]`; anything that is not the
            // public variant is treated as a private/internal helper.
            access: match f.access {
                rhai::FnAccess::Public => FnAccess::Public,
                _ => FnAccess::Private,
            },
            doc: f
                .comments
                .iter()
                .map(|c| c.trim_start_matches(['/', '!', ' ']).to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        })
        .collect();

    Ok(signatures)
}

/// Serializes the extracted signatures as a compact JSON array. On a compile
/// error the JSON object `{ "error": "<message>" }` is returned instead so the
/// supervisor/Forge always receives valid JSON.
pub fn signatures_json(source: &str) -> String {
    match extract_function_signatures(source) {
        Ok(sigs) => serde_json::to_string(&sigs).unwrap_or_else(|_| "[]".to_string()),
        Err(e) => {
            let obj = serde_json::json!({ "error": e });
            obj.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_names_and_params_in_order() {
        let src = r#"
            fn on_plugin_start() { log_info("hi"); }
            fn build_binary(target, profile) { run(target, profile) }
            private fn helper(x) { x + 1 }
        "#;
        let sigs = extract_function_signatures(src).expect("should compile");
        // Rhai does not guarantee declaration order, so look up by name.
        let by_name = |n: &str| sigs.iter().find(|s| s.name == n).expect("present");

        assert_eq!(sigs.len(), 3);

        let start = by_name("on_plugin_start");
        assert!(start.params.is_empty());
        assert_eq!(start.access, FnAccess::Public);

        let build = by_name("build_binary");
        assert_eq!(build.params, vec!["target", "profile"]);
        assert_eq!(build.access, FnAccess::Public);

        let helper = by_name("helper");
        assert_eq!(helper.params, vec!["x"]);
        assert_eq!(helper.access, FnAccess::Private);
    }

    #[test]
    fn captures_doc_comments() {
        let src = r#"
            /// Builds the release binary.
            /// Returns the artifact path.
            fn build() { "out" }
        "#;
        let sigs = extract_function_signatures(src).expect("should compile");
        let build = sigs.iter().find(|s| s.name == "build").expect("present");
        assert!(build.doc.contains("Builds the release binary."));
        assert!(build.doc.contains("Returns the artifact path."));
    }

    #[test]
    fn references_to_native_functions_compile_without_registration() {
        // `http_get` / `mcp_call_tool` are native (not registered on a bare
        // engine) yet must not break signature extraction.
        let src = r#"
            fn fetch(url) {
                let body = http_get(url);
                from_json(body)
            }
        "#;
        let sigs = extract_function_signatures(src).expect("native refs must compile");
        assert_eq!(sigs.len(), 1);
        assert_eq!(sigs[0].params, vec!["url"]);
    }

    #[test]
    fn syntax_error_surfaces_as_err() {
        let src = "fn broken( { oops";
        assert!(extract_function_signatures(src).is_err());
    }

    #[test]
    fn signatures_json_emits_error_object_on_bad_source() {
        let json = signatures_json("fn broken( {");
        assert!(json.contains("\"error\""));
    }

    #[test]
    fn signatures_json_is_array_on_success() {
        let json = signatures_json("fn a() {} fn b(x) {}");
        assert!(json.starts_with('['));
        assert!(json.contains("\"name\":\"a\"") || json.contains("\"name\": \"a\""));
    }
}
