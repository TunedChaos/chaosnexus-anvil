// chaosnexus-anvil/tests/vhai_exec_tests.rs
use chaosnexus_anvil::scripting::engine::setup_engine;
use chaosnexus_anvil::scripting::graph::canvas::{CanvasDocument, CanvasNode, CanvasWire};
use chaosnexus_anvil::scripting::graph::execute_exec_graph;
use chaosnexus_anvil::scripting::models::NativeContext;

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

fn create_test_context() -> NativeContext {
    let mut ctx = chaosnexus_anvil::scripting::engine::empty_context();
    ctx.global_state = Arc::new(RwLock::new(HashMap::new()));
    ctx
}

fn build_mock_canvas() -> CanvasDocument {
    CanvasDocument {
        version: Some(3),
        nodes: vec![
            CanvasNode {
                id: "event_1".into(),
                label: "Start".into(),
                r#fn: None,
                kind: Some("event".into()),
                r#type: None,
                value: None,
                value_type: None,
                pins: None,
                script_body: None,
                operator_id: None,
                var_name: None,
                event_id: Some("on_plugin_start".into()),
            },
            CanvasNode {
                id: "script_1".into(),
                label: "Execute Script".into(),
                r#fn: None,
                kind: Some("script".into()),
                r#type: None,
                value: None,
                value_type: None,
                pins: None,
                script_body: Some(
                    r#"
                    set_global("vhai_exec_worked", 99);
                    42
                "#
                    .into(),
                ),
                operator_id: None,
                var_name: None,
                event_id: None,
            },
        ],
        edges: vec![CanvasWire {
            id: "wire_1".into(),
            source: "event_1".into(),
            target: "script_1".into(),
            source_handle: Some("then".into()),
            target_handle: Some("exec_in".into()),
            kind: Some("exec".into()),
        }],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_execute_exec_graph() {
    let ctx = create_test_context();
    chaosnexus_anvil::scripting::models::GLOBAL_CONTEXT
        .set(ctx.clone())
        .ok();

    let engine = setup_engine(ctx.clone());
    let mut scope = chaosnexus_anvil::scripting::config_inject::build_plugin_scope("test_plugin");

    let doc = build_mock_canvas();
    let ast = rhai::AST::empty();
    let signatures = HashMap::new();

    // Run the vhai graph
    chaosnexus_anvil::scripting::plugin_context::with_plugin_context("test_plugin", || {
        let result = execute_exec_graph(&engine, &mut scope, &ast, &doc, &signatures)
            .expect("exec graph should succeed");

        // Assert the returned value
        assert_eq!(result.as_int().expect("is int"), 42);
    });

    // Verify side-effects in global_state via set_global
    let state = ctx.global_state.read().unwrap();
    let val = state
        .get("test_plugin::vhai_exec_worked")
        .expect("state mutated");
    assert_eq!(val.as_int().expect("is int"), 99);
}
