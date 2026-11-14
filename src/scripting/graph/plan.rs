// chaosnexus-anvil/src/scripting/graph/plan.rs
//
// Topological compilation for the assembly-line canvas (Phase 6b). Mirrors the
// Forge `compileTopology` contract: Kahn's algorithm over data wires, failing
// closed when a cycle is detected.

use super::canvas::{CanvasDocument, CanvasNode};
use std::collections::{HashMap, HashSet, VecDeque};

/// A compiled execution order for the canvas DAG.
#[derive(Debug, Clone)]
pub struct ExecutionPlan {
    pub order: Vec<String>,
    pub has_cycle: bool,
}

/// Returns node ids in a valid upstream-first order. On cycle, `order` is empty
/// and `has_cycle` is true.
pub fn compile_topology(doc: &CanvasDocument) -> ExecutionPlan {
    let node_ids: HashSet<String> = doc
        .nodes
        .iter()
        .filter(|n| n.r#type.as_deref() != Some("group"))
        .map(|n| n.id.clone())
        .collect();

    if node_ids.is_empty() {
        return ExecutionPlan {
            order: vec![],
            has_cycle: false,
        };
    }

    let mut indegree: HashMap<String, usize> = node_ids.iter().map(|id| (id.clone(), 0)).collect();
    let mut adjacency: HashMap<String, Vec<String>> =
        node_ids.iter().map(|id| (id.clone(), vec![])).collect();

    for wire in &doc.edges {
        if node_ids.contains(&wire.source) && node_ids.contains(&wire.target) {
            adjacency
                .get_mut(&wire.source)
                .expect("source in map")
                .push(wire.target.clone());
            *indegree.get_mut(&wire.target).expect("target in map") += 1;
        }
    }

    let declaration_order: Vec<String> = doc
        .nodes
        .iter()
        .filter(|n| node_ids.contains(&n.id))
        .map(|n| n.id.clone())
        .collect();

    let mut queue: VecDeque<String> = declaration_order
        .iter()
        .filter(|id| indegree.get(*id).copied().unwrap_or(0) == 0)
        .cloned()
        .collect();

    let mut order = Vec::new();
    while let Some(current) = queue.pop_front() {
        order.push(current.clone());
        for next in adjacency.get(&current).cloned().unwrap_or_default() {
            if let Some(deg) = indegree.get_mut(&next) {
                *deg -= 1;
                if *deg == 0 {
                    queue.push_back(next);
                }
            }
        }
    }

    if order.len() != node_ids.len() {
        return ExecutionPlan {
            order: vec![],
            has_cycle: true,
        };
    }

    ExecutionPlan {
        order,
        has_cycle: false,
    }
}

/// Forward reachability from `entry_node_id` following edge direction.
fn reachable_from(entry_node_id: &str, doc: &CanvasDocument) -> HashSet<String> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::from([entry_node_id.to_string()]);
    while let Some(id) = queue.pop_front() {
        if !visited.insert(id.clone()) {
            continue;
        }
        for wire in &doc.edges {
            if wire.source == id {
                queue.push_back(wire.target.clone());
            }
        }
    }
    visited
}

/// Collects all node ids in the execution subgraph for `entry_node_id`: the entry
/// itself, every downstream node, and every upstream ancestor that feeds them.
pub fn execution_subgraph(entry_node_id: &str, doc: &CanvasDocument) -> HashSet<String> {
    let mut set = reachable_from(entry_node_id, doc);
    loop {
        let mut added = false;
        for wire in &doc.edges {
            if set.contains(&wire.target) && set.insert(wire.source.clone()) {
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    set
}

/// Finds the first canvas node bound to `entry_fn` (by `fn` or label).
pub fn find_entry_node<'a>(doc: &'a CanvasDocument, entry_fn: &str) -> Option<&'a CanvasNode> {
    doc.nodes.iter().find(|n| {
        n.r#type.as_deref() != Some("group") && super::canvas::effective_fn(n) == Some(entry_fn)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::graph::canvas::CanvasNode;

    fn doc(nodes: Vec<(&str, &str)>, edges: Vec<(&str, &str)>) -> CanvasDocument {
        CanvasDocument {
            version: Some(2),
            nodes: nodes
                .into_iter()
                .map(|(id, label)| CanvasNode {
                    id: id.into(),
                    label: label.into(),
                    r#fn: Some(label.into()),
                    kind: None,
                    r#type: None,
                    value: None,
                    value_type: None,
                    pins: None,
                    script_body: None,
                    operator_id: None,
                    var_name: None,
                    event_id: None,
                })
                .collect(),
            edges: edges
                .into_iter()
                .enumerate()
                .map(|(i, (s, t))| super::super::canvas::CanvasWire {
                    id: format!("w{i}"),
                    source: s.into(),
                    target: t.into(),
                    source_handle: None,
                    target_handle: None,
                    kind: None,
                })
                .collect(),
        }
    }

    #[test]
    fn linear_order() {
        let d = doc(
            vec![("a", "a"), ("b", "b"), ("c", "c")],
            vec![("a", "b"), ("b", "c")],
        );
        let plan = compile_topology(&d);
        assert!(!plan.has_cycle);
        assert_eq!(plan.order, vec!["a", "b", "c"]);
    }

    #[test]
    fn cycle_detected() {
        let d = doc(vec![("a", "a"), ("b", "b")], vec![("a", "b"), ("b", "a")]);
        let plan = compile_topology(&d);
        assert!(plan.has_cycle);
        assert!(plan.order.is_empty());
    }
}
