// chaosnexus-anvil/src/scripting/graph/canvas.rs
//
// Deserializes the Forge `.chaosnexus-forge/*.canvas.json` sidecar into a typed
// topology document consumed by the assembly-line executor (Phase 6b).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A node in the assembly-line canvas (binds a canvas id to a Rhai function).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasNode {
    pub id: String,
    pub label: String,
    #[serde(default, alias = "fn")]
    pub r#fn: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub r#type: Option<String>,
    /// Literal nodes (`kind == "literal"`): the constant payload they emit.
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    /// Literal value type hint ("string" | "int" | "float" | "bool" | "json").
    #[serde(default, alias = "value_type")]
    pub value_type: Option<String>,
    /// v3: explicit pin layout.
    #[serde(default)]
    pub pins: Option<Vec<CanvasPinDescriptor>>,
    /// v3: opaque Script/Expression body.
    #[serde(default, alias = "script_body")]
    pub script_body: Option<String>,
    #[serde(default, alias = "operator_id")]
    pub operator_id: Option<String>,
    #[serde(default, alias = "var_name")]
    pub var_name: Option<String>,
    #[serde(default, alias = "event_id")]
    pub event_id: Option<String>,
}

/// A directed wire between two canvas nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasWire {
    pub id: String,
    pub source: String,
    pub target: String,
    #[serde(default, alias = "source_handle")]
    pub source_handle: Option<String>,
    #[serde(default, alias = "target_handle")]
    pub target_handle: Option<String>,
    /// v3: "exec" | "data" (defaults to data when absent).
    #[serde(default)]
    pub kind: Option<String>,
}

/// Pin descriptor persisted in v3 canvases.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasPinDescriptor {
    pub id: String,
    pub label: String,
    pub direction: String,
    #[serde(alias = "pin_kind")]
    pub pin_kind: String,
    #[serde(default, alias = "data_type")]
    pub data_type: Option<String>,
}

/// Parsed canvas sidecar (v1 or v2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasDocument {
    #[serde(default)]
    pub version: Option<u32>,
    #[serde(default)]
    pub nodes: Vec<CanvasNode>,
    #[serde(default)]
    pub edges: Vec<CanvasWire>,
}



impl CanvasDocument {
    /// Returns true when the canvas has at least one function-bound node and
    /// one wire, i.e. it describes an assembly line rather than layout-only.
    pub fn has_executable_topology(&self) -> bool {
        let has_fn_node = self.nodes.iter().any(|n| {
            n.r#type.as_deref() != Some("group")
                && (effective_fn(n).is_some() || n.kind.as_deref() == Some("event"))
                && n.kind.as_deref() != Some("branch")
                && n.kind.as_deref() != Some("iterator")
        });
        has_fn_node && !self.edges.is_empty()
    }
}

/// Resolves wire kind with v2 fallback.
pub fn wire_kind(wire: &CanvasWire) -> &'static str {
    if wire.kind.as_deref() == Some("exec") {
        "exec"
    } else {
        "data"
    }
}

/// True when the canvas has v3 exec-flow topology.
pub fn has_exec_topology(doc: &CanvasDocument) -> bool {
    doc.edges.iter().any(|w| wire_kind(w) == "exec")
        || doc.nodes.iter().any(|n| n.kind.as_deref() == Some("event"))
}

/// Resolves the Rhai function name for a canvas node (`fn` field, else `label`).
///
/// Control nodes (branch/iterator) and literal nodes have no Rhai binding and
/// return `None`.
pub fn effective_fn(node: &CanvasNode) -> Option<&str> {
    if matches!(
        node.kind.as_deref(),
        Some("branch")
            | Some("iterator")
            | Some("literal")
            | Some("event")
            | Some("sequence")
            | Some("while")
            | Some("loop")
            | Some("do-while")
            | Some("for-each")
            | Some("break")
            | Some("continue")
            | Some("return")
            | Some("try-catch")
            | Some("switch")
            | Some("get-variable")
            | Some("set-variable")
            | Some("operator")
            | Some("make-array")
            | Some("make-map")
            | Some("index")
            | Some("member-get")
            | Some("script")
            | Some("expression")
            | Some("comment")
    ) {
        return None;
    }
    node.r#fn.as_deref().or_else(|| {
        if node.r#type.as_deref() == Some("group") {
            None
        } else {
            Some(node.label.as_str())
        }
    })
}

/// Path to the sidecar for a plugin entry script:
/// `<plugin_dir>/.chaosnexus-forge/<script_basename>.canvas.json`
pub fn canvas_sidecar_path(script_path: &Path) -> PathBuf {
    let filename = script_path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    script_path
        .parent()
        .map(|p| {
            p.join(".chaosnexus-forge")
                .join(format!("{}.canvas.json", filename))
        })
        .unwrap_or_else(|| PathBuf::from(format!(".chaosnexus-forge/{}.canvas.json", filename)))
}

/// Loads and parses the canvas sidecar for `script_path`, if present.
pub fn load_canvas_sidecar(script_path: &Path) -> Option<CanvasDocument> {
    let path = canvas_sidecar_path(script_path);
    let raw = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Test helper: minimal canvas node with v3 optional fields unset.
#[cfg(test)]
pub fn test_canvas_node(
    id: &str,
    label: &str,
    fn_name: Option<&str>,
    kind: Option<&str>,
) -> CanvasNode {
    CanvasNode {
        id: id.into(),
        label: label.into(),
        r#fn: fn_name.map(str::to_string),
        kind: kind.map(str::to_string),
        r#type: None,
        value: None,
        value_type: None,
        pins: None,
        script_body: None,
        operator_id: None,
        var_name: None,
        event_id: None,
    }
}

#[cfg(test)]
pub fn test_canvas_wire(id: &str, source: &str, target: &str) -> CanvasWire {
    CanvasWire {
        id: id.into(),
        source: source.into(),
        target: target.into(),
        source_handle: None,
        target_handle: None,
        kind: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_fn_prefers_fn_field() {
        let node = CanvasNode {
            id: "n1".into(),
            label: "legacy_label".into(),
            r#fn: Some("real_fn".into()),
            kind: None,
            r#type: None,
            value: None,
            value_type: None,
            pins: None,
            script_body: None,
            operator_id: None,
            var_name: None,
            event_id: None,
        };
        assert_eq!(effective_fn(&node), Some("real_fn"));
    }

    #[test]
    fn groups_have_no_effective_fn() {
        let node = CanvasNode {
            id: "g".into(),
            label: "Main".into(),
            r#fn: None,
            kind: None,
            r#type: Some("group".into()),
            value: None,
            value_type: None,
            pins: None,
            script_body: None,
            operator_id: None,
            var_name: None,
            event_id: None,
        };
        assert_eq!(effective_fn(&node), None);
    }

    #[test]
    fn deserializes_camel_case_wires() {
        let raw = r#"{
            "version": 3,
            "displayOnly": true,
            "nodes": [{"id": "evt", "label": "start", "x": 0, "y": 0, "kind": "event", "eventId": "on_plugin_start"}],
            "edges": [{"id": "e1", "source": "evt", "target": "n2", "sourceHandle": "then", "targetHandle": "exec_in", "kind": "exec"}]
        }"#;
        let doc: CanvasDocument = serde_json::from_str(raw).expect("parse camelCase");
        assert!(doc.has_executable_topology());
        assert!(has_exec_topology(&doc));
        assert_eq!(doc.edges[0].source_handle.as_deref(), Some("then"));
    }

    #[test]
    fn deserializes_snake_case_wires_for_back_compat() {
        let raw = r#"{
            "version": 3,
            "nodes": [{"id": "n1", "label": "fn", "fn": "on_plugin_start"}],
            "edges": [{"id": "e1", "source": "n1", "target": "n2", "source_handle": "true", "target_handle": "condition"}]
        }"#;
        let doc: CanvasDocument = serde_json::from_str(raw).expect("parse snake_case");
        assert_eq!(doc.edges[0].source_handle.as_deref(), Some("true"));
    }
}
