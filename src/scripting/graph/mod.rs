// chaosnexus-anvil/src/scripting/graph/mod.rs
//
// "Assembly Line" visual scripting support (Phase 6). Splits a plugin into a
// hand-authored Rhai function library and a `.canvas.json` topology sidecar:
//
//   * `manifest` - extract function signatures from the Rhai AST so ChaosNexus Forge
//     can render node handles and detect stale function bindings.
//
// The data-gated topology executor (`execute_assembly_grid`) lands in Phase 6b.

pub mod canvas;
pub mod exec_vm;
pub mod executor;
pub mod manifest;
pub mod node_catalog;
pub mod plan;


pub use canvas::{CanvasDocument, load_canvas_sidecar};
pub use exec_vm::execute_exec_graph;
pub use executor::execute_assembly_grid;
