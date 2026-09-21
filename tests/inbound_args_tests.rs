// chaosnexus-anvil/tests/inbound_args_tests.rs
//
// Verifies that inbound MCP tool arguments are converted from `serde_json::Value`
// into native `rhai::Map`/`rhai::Array` structures (recursively) before being
// handed to a plugin's `execute(tool_name, args)` entry point.
//
// This mirrors the conversion performed in `PluginManager::handle_tool`, which
// relies on `rhai::serde::to_dynamic` so that deeply nested objects and arrays
// arrive as real Rhai values rather than stringified JSON.

use rhai::{Dynamic, Map, Scope};

/// Builds a Rhai argument map from a JSON object exactly the way
/// `handle_tool` does, so the test stays faithful to production behavior.
fn rhai_args_from_json(args: &serde_json::Map<String, serde_json::Value>) -> Map {
    let mut rhai_args = Map::new();
    for (k, v) in args {
        let dynamic = rhai::serde::to_dynamic(v).unwrap_or_else(|_| Dynamic::from(v.to_string()));
        rhai_args.insert(k.clone().into(), dynamic);
    }
    rhai_args
}

#[test]
fn inbound_args_expose_deep_structural_trees_natively() {
    let ctx = chaosnexus_anvil::scripting::engine::empty_context();
    let engine = chaosnexus_anvil::scripting::engine::setup_engine(ctx);

    // A deliberately deep tree: object -> object -> array -> object, plus a
    // top-level array of scalars and mixed scalar types.
    let payload = serde_json::json!({
        "name": "deploy",
        "count": 3,
        "enabled": true,
        "config": {
            "region": "us-east",
            "replicas": [
                { "id": 1, "healthy": true },
                { "id": 2, "healthy": false }
            ],
            "labels": ["a", "b", "c"]
        }
    });

    let args = rhai_args_from_json(payload.as_object().expect("payload is a JSON object"));

    let mut scope = Scope::new();
    scope.push("args", args);

    // The script indexes natively through maps and arrays. If any level were
    // stringified JSON, these accessors would fail to type-check at runtime.
    let script = r#"
        let region = args.config.region;
        let second_id = args.config.replicas[1].id;
        let second_healthy = args.config.replicas[1].healthy;
        let label = args.config.labels[2];
        let total = args.count;
        `${region}|${second_id}|${second_healthy}|${label}|${total}`
    "#;

    let result = engine
        .eval_with_scope::<String>(&mut scope, script)
        .expect("nested native access should succeed");

    assert_eq!(result, "us-east|2|false|c|3");
}

#[test]
fn inbound_scalar_string_arg_is_native_string() {
    let ctx = chaosnexus_anvil::scripting::engine::empty_context();
    let engine = chaosnexus_anvil::scripting::engine::setup_engine(ctx);

    let payload = serde_json::json!({ "message": "hello" });
    let args = rhai_args_from_json(payload.as_object().unwrap());

    let mut scope = Scope::new();
    scope.push("args", args);

    // A plain string must arrive unquoted (not as a JSON-encoded literal).
    let result = engine
        .eval_with_scope::<String>(&mut scope, "args.message")
        .expect("string arg should be a native string");

    assert_eq!(result, "hello");
}
