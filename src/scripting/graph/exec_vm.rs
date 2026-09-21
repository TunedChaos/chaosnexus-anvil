// chaosnexus-anvil/src/scripting/graph/exec_vm.rs
//
// Vhai exec-flow VM (Phase 7). Walks white execution wires from Event
// nodes, pulls typed data inputs on demand, and dispatches native control nodes
// or Rhai function calls. Leaf logic uses micro-AST eval for operators and
// opaque script/expression blocks.

use super::canvas::{CanvasDocument, CanvasNode, CanvasWire, effective_fn, wire_kind};
use super::node_catalog::{
    self, EXEC_BODY, EXEC_CATCH, EXEC_COMPLETED, EXEC_FALSE, EXEC_OUT, EXEC_TRUE, KIND_BRANCH,
    KIND_BREAK, KIND_COMMENT, KIND_CONTINUE, KIND_DO_WHILE, KIND_EVENT, KIND_EXPRESSION,
    KIND_FOR_EACH, KIND_GET_VARIABLE, KIND_INDEX, KIND_LITERAL, KIND_LOOP, KIND_MAKE_ARRAY,
    KIND_MAKE_MAP, KIND_MEMBER_GET, KIND_OPERATOR, KIND_RETURN, KIND_SCRIPT, KIND_SEQUENCE,
    KIND_SET_VARIABLE, KIND_SWITCH, KIND_TRY_CATCH, KIND_WHILE, RETURN_HANDLE,
};
use rhai::{AST, Dynamic, Engine, Scope};
use std::collections::HashMap;

/// Flow-control signal from Break/Continue/Return nodes.
#[derive(Debug, Clone)]
enum FlowSignal {
    Break,
    Continue,
    Return(Dynamic),
    None,
}

#[allow(dead_code)]
struct LoopFrame {
    break_target: Option<String>,
    continue_target: Option<String>,
}

struct VmState {
    outputs: HashMap<String, Dynamic>,
    loop_stack: Vec<LoopFrame>,
    flow: FlowSignal,
    last_value: Dynamic,
}

/// Returns the kind of the canvas node, defaulting to "function" if absent.
fn node_kind(node: &CanvasNode) -> &str {
    node.kind.as_deref().unwrap_or("function")
}

/// Retrieves outgoing execution wires originating from `source_id`.
/// If `handle` is provided, filters the wires to those matching `source_handle`.
fn exec_edges_from<'a>(
    doc: &'a CanvasDocument,
    source_id: &str,
    handle: Option<&str>,
) -> Vec<&'a CanvasWire> {
    doc.edges
        .iter()
        .filter(|w| {
            wire_kind(w) == "exec"
                && w.source == source_id
                && handle.is_none_or(|h| w.source_handle.as_deref() == Some(h))
        })
        .collect()
}

/// Retrieves all incoming data wires directed to `target_id`.
fn data_edges_to<'a>(doc: &'a CanvasDocument, target_id: &str) -> Vec<&'a CanvasWire> {
    doc.edges
        .iter()
        .filter(|w| wire_kind(w) == "data" && w.target == target_id)
        .collect()
}

/// Evaluates a small Rhai expression snippet with the provided bindings pushed into the scope.
fn eval_expression_snippet(
    engine: &Engine,
    scope: &mut Scope,
    expr: &str,
    bindings: &HashMap<String, Dynamic>,
) -> Result<Dynamic, String> {
    let push_count = bindings.len();
    for (k, v) in bindings {
        scope.push(k.clone(), v.clone());
    }
    let wrapped = format!("{{ {expr} }}");
    let result = match engine.eval_with_scope(scope, &wrapped) {
        Ok(v) => Ok(v),
        Err(e) => Err(e.to_string()),
    };
    for _ in 0..push_count {
        scope.pop();
    }
    result
}

/// Evaluates a block of Rhai script and returns its result.
fn eval_script_block(engine: &Engine, scope: &mut Scope, body: &str) -> Result<Dynamic, String> {
    match engine.eval_with_scope(scope, body) {
        Ok(v) => Ok(v),
        Err(e) => Err(e.to_string()),
    }
}

/// Collects evaluated incoming data for a node into a map keyed by the target handle name.
fn gather_data_map(
    engine: &Engine,
    scope: &mut Scope,
    ast: &AST,
    doc: &CanvasDocument,
    vm: &mut VmState,
    signatures: &HashMap<String, Vec<String>>,
    node_id: &str,
) -> Result<HashMap<String, Dynamic>, String> {
    let mut map = HashMap::new();
    for wire in data_edges_to(doc, node_id) {
        let key = wire
            .target_handle
            .clone()
            .unwrap_or_else(|| RETURN_HANDLE.to_string());
        let val = evaluate_data_node(engine, scope, ast, doc, vm, signatures, &wire.source)?;
        map.insert(key, val);
    }
    Ok(map)
}

/// Evaluates a data-producing node by processing its inputs and returning the computed value.
fn evaluate_data_node(
    engine: &Engine,
    scope: &mut Scope,
    ast: &AST,
    doc: &CanvasDocument,
    vm: &mut VmState,
    signatures: &HashMap<String, Vec<String>>,
    node_id: &str,
) -> Result<Dynamic, String> {
    if let Some(cached) = vm.outputs.get(node_id) {
        return Ok(cached.clone());
    }
    let node = doc
        .nodes
        .iter()
        .find(|n| n.id == node_id)
        .ok_or_else(|| format!("Node '{node_id}' not found"))?;

    let value = match node_kind(node) {
        KIND_LITERAL => node
            .value
            .as_ref()
            .and_then(|v| rhai::serde::to_dynamic(v).ok())
            .unwrap_or(Dynamic::UNIT),
        KIND_GET_VARIABLE => {
            let name = node.var_name.as_deref().unwrap_or("my_var");
            match scope.get_value::<Dynamic>(name) {
                Some(v) => v.clone(),
                None => Dynamic::UNIT,
            }
        }
        KIND_OPERATOR => {
            let op = node.operator_id.as_deref().unwrap_or("add");
            let gathered = gather_data_map(engine, scope, ast, doc, vm, signatures, node_id)?;
            let expr = node_catalog::operator_expression(op)
                .ok_or_else(|| format!("Unknown operator '{op}'"))?;
            eval_expression_snippet(engine, scope, expr, &gathered)?
        }
        KIND_EXPRESSION => {
            let body = node.script_body.as_deref().unwrap_or("()");
            let gathered = gather_data_map(engine, scope, ast, doc, vm, signatures, node_id)?;
            eval_expression_snippet(engine, scope, body, &gathered)?
        }
        KIND_MAKE_ARRAY => {
            let gathered = gather_data_map(engine, scope, ast, doc, vm, signatures, node_id)?;
            let mut arr = rhai::Array::new();
            for key in ["elem_0", "elem_1"] {
                if let Some(v) = gathered.get(key) {
                    arr.push(v.clone());
                }
            }
            Dynamic::from_array(arr)
        }
        KIND_MAKE_MAP => {
            let gathered = gather_data_map(engine, scope, ast, doc, vm, signatures, node_id)?;
            let mut map = rhai::Map::new();
            if let (Some(k), Some(v)) = (gathered.get("key"), gathered.get("value")) {
                let key = k.to_string();
                map.insert(key.into(), v.clone());
            }
            Dynamic::from(map)
        }
        KIND_INDEX => {
            let gathered = gather_data_map(engine, scope, ast, doc, vm, signatures, node_id)?;
            let target = gathered.get("target").cloned().unwrap_or(Dynamic::UNIT);
            let idx = gathered.get("index").cloned().unwrap_or(Dynamic::UNIT);
            if let Some(arr) = target.clone().try_cast::<rhai::Array>() {
                let i = idx.as_int().ok().unwrap_or(0) as usize;
                arr.get(i).cloned().unwrap_or(Dynamic::UNIT)
            } else if let Some(map) = target.clone().try_cast::<rhai::Map>() {
                let key = idx.to_string();
                map.get(key.as_str()).cloned().unwrap_or(Dynamic::UNIT)
            } else {
                Dynamic::UNIT
            }
        }
        KIND_MEMBER_GET => {
            let gathered = gather_data_map(engine, scope, ast, doc, vm, signatures, node_id)?;
            let obj = gathered.get("object").cloned().unwrap_or(Dynamic::UNIT);
            let member = gathered
                .get("member")
                .and_then(|m| m.clone().into_string().ok())
                .unwrap_or_default();
            if let Some(map) = obj.try_cast::<rhai::Map>() {
                map.get(member.as_str()).cloned().unwrap_or(Dynamic::UNIT)
            } else {
                Dynamic::UNIT
            }
        }
        _ => {
            if let Some(fn_name) = effective_fn(node) {
                let param_names = signatures.get(fn_name).cloned().unwrap_or_default();
                let gathered = gather_data_map(engine, scope, ast, doc, vm, signatures, node_id)?;
                let args: Vec<Dynamic> = param_names
                    .iter()
                    .map(|p| gathered.get(p).cloned().unwrap_or(Dynamic::UNIT))
                    .collect();
                super::executor::call_fn_dynamic(engine, scope, ast, fn_name, args)?
            } else {
                Dynamic::UNIT
            }
        }
    };
    vm.outputs.insert(node_id.to_string(), value.clone());
    Ok(value)
}

/// Executes an execution node based on its kind, updating the VM state and returning the next node ID to execute, if any.
fn run_exec_node(
    engine: &Engine,
    scope: &mut Scope,
    ast: &AST,
    doc: &CanvasDocument,
    vm: &mut VmState,
    signatures: &HashMap<String, Vec<String>>,
    node_id: &str,
) -> Result<Option<String>, String> {
    let node = doc
        .nodes
        .iter()
        .find(|n| n.id == node_id)
        .ok_or_else(|| format!("Node '{node_id}' not found"))?;

    let kind = node_kind(node);
    vm.flow = FlowSignal::None;

    match kind {
        KIND_COMMENT => {}
        KIND_SET_VARIABLE => {
            let gathered = gather_data_map(engine, scope, ast, doc, vm, signatures, node_id)?;
            let name = node.var_name.as_deref().unwrap_or("my_var");
            let val = gathered.get("value").cloned().unwrap_or(Dynamic::UNIT);
            scope.set_value(name, val);
        }
        KIND_SCRIPT => {
            let body = node.script_body.as_deref().unwrap_or("");
            let result = eval_script_block(engine, scope, body)?;
            vm.last_value = result.clone();
            vm.outputs.insert(node_id.to_string(), result);
        }
        KIND_BREAK => {
            vm.flow = FlowSignal::Break;
            return Ok(None);
        }
        KIND_CONTINUE => {
            vm.flow = FlowSignal::Continue;
            return Ok(None);
        }
        KIND_RETURN => {
            let gathered = gather_data_map(engine, scope, ast, doc, vm, signatures, node_id)?;
            let val = gathered.get("value").cloned().unwrap_or(Dynamic::UNIT);
            vm.flow = FlowSignal::Return(val.clone());
            vm.last_value = val;
            return Ok(None);
        }
        KIND_SEQUENCE => {
            for handle in ["then_0", "then_1"] {
                for wire in exec_edges_from(doc, node_id, Some(handle)) {
                    run_exec_chain(engine, scope, ast, doc, vm, signatures, &wire.target)?;
                    if !matches!(vm.flow, FlowSignal::None) {
                        return Ok(None);
                    }
                }
            }
            return Ok(exec_edges_from(doc, node_id, Some(EXEC_OUT))
                .first()
                .map(|w| w.target.clone()));
        }
        KIND_BRANCH => {
            let gathered = gather_data_map(engine, scope, ast, doc, vm, signatures, node_id)?;
            let cond = gathered
                .get("condition")
                .and_then(|d| d.as_bool().ok())
                .unwrap_or(false);
            let lane = if cond { EXEC_TRUE } else { EXEC_FALSE };
            return Ok(exec_edges_from(doc, node_id, Some(lane))
                .first()
                .map(|w| w.target.clone()));
        }
        KIND_SWITCH => {
            return Ok(exec_edges_from(doc, node_id, Some("default"))
                .first()
                .map(|w| w.target.clone()));
        }
        KIND_WHILE | KIND_LOOP | KIND_DO_WHILE | KIND_FOR_EACH => {
            let body_handle = if kind == KIND_FOR_EACH {
                "item"
            } else {
                EXEC_BODY
            };
            let body_entry = exec_edges_from(doc, node_id, Some(body_handle))
                .first()
                .map(|w| w.target.clone());
            let completed = exec_edges_from(doc, node_id, Some(EXEC_COMPLETED))
                .first()
                .map(|w| w.target.clone());

            vm.loop_stack.push(LoopFrame {
                break_target: completed.clone(),
                continue_target: body_entry.clone(),
            });

            macro_rules! handle_loop_flow {
                () => {
                    match &vm.flow {
                        FlowSignal::Break => {
                            vm.flow = FlowSignal::None;
                            break;
                        }
                        FlowSignal::Continue => {
                            vm.flow = FlowSignal::None;
                            continue;
                        }
                        FlowSignal::Return(_) => {
                            vm.loop_stack.pop();
                            return Ok(None);
                        }
                        FlowSignal::None => {}
                    }
                };
            }

            if kind == KIND_FOR_EACH {
                let gathered = gather_data_map(engine, scope, ast, doc, vm, signatures, node_id)?;
                let items = gathered
                    .get("items")
                    .and_then(|d| d.clone().try_cast::<rhai::Array>())
                    .unwrap_or_default();
                for (i, item) in items.into_iter().enumerate() {
                    scope.set_value("_foreach_item", item.clone());
                    scope.set_value("_foreach_index", Dynamic::from(i as i64));
                    if let Some(ref entry) = body_entry {
                        run_exec_chain(engine, scope, ast, doc, vm, signatures, entry)?;
                        handle_loop_flow!();
                    }
                }
            } else if kind == KIND_WHILE {
                loop {
                    let gathered =
                        gather_data_map(engine, scope, ast, doc, vm, signatures, node_id)?;
                    let cond = gathered
                        .get("condition")
                        .and_then(|d| d.as_bool().ok())
                        .unwrap_or(false);
                    if !cond {
                        break;
                    }
                    if let Some(ref entry) = body_entry {
                        run_exec_chain(engine, scope, ast, doc, vm, signatures, entry)?;
                        handle_loop_flow!();
                    }
                }
            } else if kind == KIND_LOOP {
                loop {
                    if let Some(ref entry) = body_entry {
                        run_exec_chain(engine, scope, ast, doc, vm, signatures, entry)?;
                        handle_loop_flow!();
                    }
                }
            } else if kind == KIND_DO_WHILE {
                loop {
                    if let Some(ref entry) = body_entry {
                        run_exec_chain(engine, scope, ast, doc, vm, signatures, entry)?;
                        handle_loop_flow!();
                    }
                    let gathered =
                        gather_data_map(engine, scope, ast, doc, vm, signatures, node_id)?;
                    let cond = gathered
                        .get("condition")
                        .and_then(|d| d.as_bool().ok())
                        .unwrap_or(false);
                    if !cond {
                        break;
                    }
                }
            }

            vm.loop_stack.pop();
            return Ok(completed);
        }
        KIND_TRY_CATCH => {
            let try_entry = exec_edges_from(doc, node_id, Some("try"))
                .first()
                .map(|w| w.target.clone());
            let catch_entry = exec_edges_from(doc, node_id, Some(EXEC_CATCH))
                .first()
                .map(|w| w.target.clone());
            if let Some(ref entry) = try_entry
                && let Err(e) = run_exec_chain(engine, scope, ast, doc, vm, signatures, entry)
            {
                scope.set_value("_last_error", e);
                if let Some(ref catch) = catch_entry {
                    run_exec_chain(engine, scope, ast, doc, vm, signatures, catch)?;
                }
            }
            return Ok(exec_edges_from(doc, node_id, Some(EXEC_OUT))
                .first()
                .map(|w| w.target.clone()));
        }
        _ => {
            if let Some(fn_name) = effective_fn(node) {
                let param_names = signatures.get(fn_name).cloned().unwrap_or_default();
                let gathered = gather_data_map(engine, scope, ast, doc, vm, signatures, node_id)?;
                let args: Vec<Dynamic> = param_names
                    .iter()
                    .map(|p| gathered.get(p).cloned().unwrap_or(Dynamic::UNIT))
                    .collect();
                let result = super::executor::call_fn_dynamic(engine, scope, ast, fn_name, args)?;
                vm.last_value = result.clone();
                vm.outputs.insert(node_id.to_string(), result);
            }
        }
    }

    Ok(exec_edges_from(doc, node_id, Some(EXEC_OUT))
        .first()
        .map(|w| w.target.clone()))
}

/// Iteratively runs a chain of execution nodes starting from `start_id`.
fn run_exec_chain(
    engine: &Engine,
    scope: &mut Scope,
    ast: &AST,
    doc: &CanvasDocument,
    vm: &mut VmState,
    signatures: &HashMap<String, Vec<String>>,
    start_id: &str,
) -> Result<(), String> {
    let mut current = Some(start_id.to_string());
    while let Some(node_id) = current.take() {
        if matches!(vm.flow, FlowSignal::Return(_)) {
            return Ok(());
        }
        current = run_exec_node(engine, scope, ast, doc, vm, signatures, &node_id)?;
        if matches!(vm.flow, FlowSignal::Break | FlowSignal::Continue) {
            return Ok(());
        }
    }
    Ok(())
}

/// Finds the first Event node (exec root).
pub fn find_event_node(doc: &CanvasDocument) -> Option<&CanvasNode> {
    doc.nodes.iter().find(|n| node_kind(n) == KIND_EVENT)
}

/// Executes a Vhai exec-flow graph from its Event node.
pub fn execute_exec_graph(
    engine: &Engine,
    scope: &mut Scope,
    ast: &AST,
    doc: &CanvasDocument,
    signatures: &HashMap<String, Vec<String>>,
) -> Result<Dynamic, String> {
    let event = find_event_node(doc).ok_or("No Event node found in canvas")?;
    let entry = exec_edges_from(doc, &event.id, Some("then"))
        .first()
        .map(|w| w.target.clone())
        .ok_or("Event node has no 'then' execution wire")?;

    let mut vm = VmState {
        outputs: HashMap::new(),
        loop_stack: vec![],
        flow: FlowSignal::None,
        last_value: Dynamic::UNIT,
    };

    run_exec_chain(engine, scope, ast, doc, &mut vm, signatures, &entry)?;

    if let FlowSignal::Return(v) = vm.flow {
        return Ok(v);
    }
    Ok(vm.last_value)
}
