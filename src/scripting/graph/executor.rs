// chaosnexus-anvil/src/scripting/graph/executor.rs
//
// Data-gated assembly-line executor (Phase 6b). Walks the canvas DAG in
// topological order, threading return payloads along wires. A node fires once
// every inbound parameter handle is satisfied (the Stream model).

use super::canvas::{CanvasDocument, CanvasNode, CanvasWire, effective_fn, has_exec_topology};
use super::plan::{compile_topology, execution_subgraph, find_entry_node};
use rhai::{AST, Dynamic, Engine, Map, Scope};
use std::collections::HashMap;

/// Outbound return handle id (mirrors Forge `RETURN_HANDLE`).
pub const RETURN_HANDLE: &str = "return";

/// Branch router output handles.
const BRANCH_TRUE: &str = "true";
const BRANCH_FALSE: &str = "false";

/// Iterator map input / per-item output handles.
const ITER_ITEMS: &str = "items";
const ITER_ITEM: &str = "item";

#[derive(Debug, Clone)]
enum NodeOutput {
    /// A Rhai function return payload.
    Value(Dynamic),
    /// Branch router state: payload routed to `true` or `false` lane only.
    Branch { payload: Dynamic, route_true: bool },
}

/// Invokes a Rhai function with a dynamically-sized argument list.
pub fn call_fn_dynamic(
    engine: &Engine,
    scope: &mut Scope,
    ast: &AST,
    fn_name: &str,
    args: Vec<Dynamic>,
) -> Result<Dynamic, String> {
    let res = match args.len() {
        0 => engine.call_fn(scope, ast, fn_name, ()),
        1 => engine.call_fn(scope, ast, fn_name, (args[0].clone(),)),
        2 => engine.call_fn(scope, ast, fn_name, (args[0].clone(), args[1].clone())),
        3 => engine.call_fn(
            scope,
            ast,
            fn_name,
            (args[0].clone(), args[1].clone(), args[2].clone()),
        ),
        4 => engine.call_fn(
            scope,
            ast,
            fn_name,
            (
                args[0].clone(),
                args[1].clone(),
                args[2].clone(),
                args[3].clone(),
            ),
        ),
        5 => engine.call_fn(
            scope,
            ast,
            fn_name,
            (
                args[0].clone(),
                args[1].clone(),
                args[2].clone(),
                args[3].clone(),
                args[4].clone(),
            ),
        ),
        6 => engine.call_fn(
            scope,
            ast,
            fn_name,
            (
                args[0].clone(),
                args[1].clone(),
                args[2].clone(),
                args[3].clone(),
                args[4].clone(),
                args[5].clone(),
            ),
        ),
        n => {
            return Err(format!(
                "Function '{fn_name}' has {n} arguments; assembly grid supports up to 6"
            ));
        }
    };
    res.map_err(|e| e.to_string())
}

/// Determines the kind of a canvas node, defaulting to "function" if not specified.
fn node_kind(node: &CanvasNode) -> &str {
    node.kind.as_deref().unwrap_or("function")
}

/// Retrieves all incoming wires that target a specific node ID.
fn inbound_wires<'a>(doc: &'a CanvasDocument, target_id: &str) -> Vec<&'a CanvasWire> {
    doc.edges.iter().filter(|w| w.target == target_id).collect()
}

/// Retrieves all outgoing wires that originate from a specific node ID.
fn outbound_wires<'a>(doc: &'a CanvasDocument, source_id: &str) -> Vec<&'a CanvasWire> {
    doc.edges.iter().filter(|w| w.source == source_id).collect()
}

/// Resolves the payload carried by `wire` from a previously executed `outputs` map.
fn resolve_wire_value(wire: &CanvasWire, outputs: &HashMap<String, NodeOutput>) -> Option<Dynamic> {
    let source = outputs.get(&wire.source)?;
    match source {
        NodeOutput::Value(v) => Some(v.clone()),
        NodeOutput::Branch {
            payload,
            route_true,
        } => {
            let handle = wire
                .source_handle
                .as_deref()
                .unwrap_or(if *route_true {
                    BRANCH_TRUE
                } else {
                    BRANCH_FALSE
                })
                .to_ascii_lowercase();
            let want_true = handle == BRANCH_TRUE;
            if want_true == *route_true {
                Some(payload.clone())
            } else {
                None
            }
        }
    }
}

/// Gathers inbound parameter values for a function node keyed by parameter name.
fn gather_param_map(
    doc: &CanvasDocument,
    node_id: &str,
    outputs: &HashMap<String, NodeOutput>,
    entry_inject: &HashMap<String, Dynamic>,
) -> HashMap<String, Dynamic> {
    let mut params = HashMap::new();
    for wire in inbound_wires(doc, node_id) {
        if let Some(value) = resolve_wire_value(wire, outputs) {
            let key = wire
                .target_handle
                .clone()
                .unwrap_or_else(|| RETURN_HANDLE.to_string());
            params.insert(key, value);
        }
    }
    for (k, v) in entry_inject {
        params.entry(k.clone()).or_insert_with(|| v.clone());
    }
    params
}

/// Builds positional args for a Rhai call from a parameter name list and gathered values.
fn positional_args(param_names: &[String], gathered: &HashMap<String, Dynamic>) -> Vec<Dynamic> {
    param_names
        .iter()
        .map(|name| gathered.get(name).cloned().unwrap_or(Dynamic::UNIT))
        .collect()
}

/// Executes one function node and stores its return payload.
#[allow(clippy::too_many_arguments)]
fn run_function_node(
    engine: &Engine,
    scope: &mut Scope,
    ast: &AST,
    node: &CanvasNode,
    fn_name: &str,
    param_names: &[String],
    gathered: &HashMap<String, Dynamic>,
    outputs: &mut HashMap<String, NodeOutput>,
) -> Result<(), String> {
    let args = positional_args(param_names, gathered);
    let result = call_fn_dynamic(engine, scope, ast, fn_name, args)?;
    outputs.insert(node.id.clone(), NodeOutput::Value(result));
    Ok(())
}

/// Runs immediate downstream targets wired from an iterator's `item` handle once per element.
#[allow(clippy::too_many_arguments)]
fn run_iterator_fanout(
    engine: &Engine,
    scope: &mut Scope,
    ast: &AST,
    doc: &CanvasDocument,
    node: &CanvasNode,
    items: rhai::Array,
    signatures: &HashMap<String, Vec<String>>,
    _outputs: &mut HashMap<String, NodeOutput>,
) -> Result<Dynamic, String> {
    let item_wires: Vec<&CanvasWire> = outbound_wires(doc, &node.id)
        .into_iter()
        .filter(|w| {
            w.source_handle
                .as_deref()
                .map(|h| h == ITER_ITEM)
                .unwrap_or(false)
        })
        .collect();

    let mut collected = rhai::Array::new();
    for item in items {
        for wire in &item_wires {
            let Some(target) = doc.nodes.iter().find(|n| n.id == wire.target) else {
                continue;
            };
            if node_kind(target) != "function" {
                continue;
            }
            let Some(fn_name) = effective_fn(target).map(str::to_string) else {
                continue;
            };
            let param_names = signatures.get(&fn_name).cloned().unwrap_or_default();
            let mut gathered = HashMap::new();
            if let Some(handle) = &wire.target_handle {
                gathered.insert(handle.clone(), item.clone());
            } else if let Some(first) = param_names.first() {
                gathered.insert(first.clone(), item.clone());
            } else {
                gathered.insert(RETURN_HANDLE.to_string(), item.clone());
            }
            let args = positional_args(&param_names, &gathered);
            let result = call_fn_dynamic(engine, scope, ast, &fn_name, args)?;
            collected.push(result);
        }
    }
    Ok(Dynamic::from_array(collected))
}

/// Executes the assembly grid for `entry_fn`, injecting `entry_args` into the entry node.
///
/// Returns the final return payload from the last executed node in topo order within the
/// reachable subgraph, or the entry node's own return when it is the only node.
pub fn execute_assembly_grid(
    engine: &Engine,
    scope: &mut Scope,
    ast: &AST,
    doc: &CanvasDocument,
    signatures: &HashMap<String, Vec<String>>,
    entry_fn: &str,
    entry_args: Map,
) -> Result<Dynamic, String> {
    if !doc.has_executable_topology() && !has_exec_topology(doc) {
        return Err("Canvas has no executable topology".into());
    }

    if has_exec_topology(doc) {
        return super::exec_vm::execute_exec_graph(engine, scope, ast, doc, signatures);
    }

    let plan = compile_topology(doc);
    if plan.has_cycle {
        return Err("Assembly grid contains a cycle".into());
    }

    let entry_node = find_entry_node(doc, entry_fn)
        .ok_or_else(|| format!("No canvas node bound to entry function '{entry_fn}'"))?;

    let reachable = execution_subgraph(&entry_node.id, doc);
    let mut outputs: HashMap<String, NodeOutput> = HashMap::new();

    let mut entry_inject: HashMap<String, Dynamic> = HashMap::new();
    for (k, v) in entry_args {
        entry_inject.insert(k.to_string(), v);
    }

    let mut last_value = Dynamic::UNIT;

    for node_id in &plan.order {
        if !reachable.contains(node_id) {
            continue;
        }
        let Some(node) = doc.nodes.iter().find(|n| n.id == *node_id) else {
            continue;
        };
        if node.r#type.as_deref() == Some("group") {
            continue;
        }

        let inject = if node_id == &entry_node.id {
            &entry_inject
        } else {
            &HashMap::new()
        };

        match node_kind(node) {
            "literal" => {
                // Constant source: convert the stored JSON value into a native
                // Rhai `Dynamic` (strings, numbers, bools, arrays, maps).
                let value = node
                    .value
                    .as_ref()
                    .and_then(|v| rhai::serde::to_dynamic(v).ok())
                    .unwrap_or(Dynamic::UNIT);
                last_value = value.clone();
                outputs.insert(node.id.clone(), NodeOutput::Value(value));
            }
            "branch" => {
                let gathered = gather_param_map(doc, &node.id, &outputs, inject);
                let condition = gathered
                    .get("condition")
                    .and_then(|d| d.as_bool().ok())
                    .unwrap_or(false);
                let payload = gathered.get("payload").cloned().unwrap_or(Dynamic::UNIT);
                outputs.insert(
                    node.id.clone(),
                    NodeOutput::Branch {
                        payload: payload.clone(),
                        route_true: condition,
                    },
                );
                last_value = payload;
            }
            "iterator" => {
                let gathered = gather_param_map(doc, &node.id, &outputs, inject);
                let items = gathered
                    .get(ITER_ITEMS)
                    .and_then(|d| d.clone().try_cast::<rhai::Array>())
                    .unwrap_or_default();
                let result = run_iterator_fanout(
                    engine,
                    scope,
                    ast,
                    doc,
                    node,
                    items,
                    signatures,
                    &mut outputs,
                )?;
                outputs.insert(node.id.clone(), NodeOutput::Value(result.clone()));
                last_value = result;
            }
            _ => {
                let Some(fn_name) = effective_fn(node).map(str::to_string) else {
                    continue;
                };
                let param_names = signatures.get(&fn_name).cloned().unwrap_or_default();
                let gathered = gather_param_map(doc, &node.id, &outputs, inject);

                // Stream gate: skip nodes whose required inbound wires are unsatisfied.
                let inbound = inbound_wires(doc, &node.id);
                if !inbound.is_empty() {
                    let mut satisfied = true;
                    for wire in &inbound {
                        if resolve_wire_value(wire, &outputs).is_none() {
                            satisfied = false;
                            break;
                        }
                    }
                    if !satisfied && node_id != &entry_node.id {
                        continue;
                    }
                }

                run_function_node(
                    engine,
                    scope,
                    ast,
                    node,
                    &fn_name,
                    &param_names,
                    &gathered,
                    &mut outputs,
                )?;
                if let Some(NodeOutput::Value(v)) = outputs.get(&node.id) {
                    last_value = v.clone();
                }
            }
        }
    }

    Ok(last_value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::graph::canvas::{CanvasDocument, CanvasWire, test_canvas_node};
    use crate::scripting::graph::manifest::extract_function_signatures;

    fn test_doc() -> CanvasDocument {
        CanvasDocument {
            version: Some(2),
            nodes: vec![
                test_canvas_node("n_a", "step_a", Some("step_a"), None),
                test_canvas_node("n_b", "step_b", Some("step_b"), None),
            ],
            edges: vec![CanvasWire {
                id: "w1".into(),
                source: "n_a".into(),
                target: "n_b".into(),
                source_handle: Some(RETURN_HANDLE.into()),
                target_handle: Some("input".into()),
                kind: None,
            }],
        }
    }

    #[test]
    fn executes_linear_pipeline() {
        let src = r#"
            fn step_a() { 10 }
            fn step_b(input) { input + 5 }
        "#;
        let engine = Engine::new();
        let ast = engine.compile(src).expect("compile");
        let sigs: HashMap<String, Vec<String>> = extract_function_signatures(src)
            .expect("sigs")
            .into_iter()
            .map(|s| (s.name, s.params))
            .collect();

        let result = execute_assembly_grid(
            &engine,
            &mut Scope::new(),
            &ast,
            &test_doc(),
            &sigs,
            "step_a",
            Map::new(),
        )
        .expect("grid run");

        assert_eq!(result.as_int().ok(), Some(15));
    }

    #[test]
    fn literal_feeds_downstream_function() {
        let src = r#"fn shout(msg) { msg + "!" }"#;
        let mut lit = test_canvas_node("lit", "literal", None, Some("literal"));
        lit.value = Some(serde_json::json!("hello"));
        lit.value_type = Some("string".into());
        let doc = CanvasDocument {
            version: Some(2),
            nodes: vec![lit, test_canvas_node("fn1", "shout", Some("shout"), None)],
            edges: vec![CanvasWire {
                id: "w1".into(),
                source: "lit".into(),
                target: "fn1".into(),
                source_handle: Some(RETURN_HANDLE.into()),
                target_handle: Some("msg".into()),
                kind: None,
            }],
        };

        let engine = Engine::new();
        let ast = engine.compile(src).expect("compile");
        let sigs: HashMap<String, Vec<String>> = extract_function_signatures(src)
            .expect("sigs")
            .into_iter()
            .map(|s| (s.name, s.params))
            .collect();

        let result = execute_assembly_grid(
            &engine,
            &mut Scope::new(),
            &ast,
            &doc,
            &sigs,
            "shout",
            Map::new(),
        )
        .expect("literal grid");

        assert_eq!(result.into_string().ok().as_deref(), Some("hello!"));
    }

    #[test]
    fn branch_routes_true_lane_only() {
        let src = r#"
            fn cond() { true }
            fn payload() { "ok" }
            fn on_true(msg) { "T:" + msg }
            fn on_false(msg) { "F:" + msg }
        "#;
        let doc = CanvasDocument {
            version: Some(2),
            nodes: vec![
                test_canvas_node("c", "cond", Some("cond"), None),
                test_canvas_node("p", "payload", Some("payload"), None),
                test_canvas_node("br", "branch", None, Some("branch")),
                test_canvas_node("t", "on_true", Some("on_true"), None),
            ],
            edges: vec![
                CanvasWire {
                    id: "w1".into(),
                    source: "p".into(),
                    target: "br".into(),
                    source_handle: Some(RETURN_HANDLE.into()),
                    target_handle: Some("payload".into()),
                    kind: None,
                },
                CanvasWire {
                    id: "w2".into(),
                    source: "c".into(),
                    target: "br".into(),
                    source_handle: Some(RETURN_HANDLE.into()),
                    target_handle: Some("condition".into()),
                    kind: None,
                },
                CanvasWire {
                    id: "w3".into(),
                    source: "br".into(),
                    target: "t".into(),
                    source_handle: Some(BRANCH_TRUE.into()),
                    target_handle: Some("msg".into()),
                    kind: None,
                },
            ],
        };

        let engine = Engine::new();
        let ast = engine.compile(src).expect("compile");
        let sigs: HashMap<String, Vec<String>> = extract_function_signatures(src)
            .expect("sigs")
            .into_iter()
            .map(|s| (s.name, s.params))
            .collect();

        let result = execute_assembly_grid(
            &engine,
            &mut Scope::new(),
            &ast,
            &doc,
            &sigs,
            "cond",
            Map::new(),
        )
        .expect("branch grid");

        assert_eq!(result.into_string().ok().as_deref(), Some("T:ok"));
    }
}
