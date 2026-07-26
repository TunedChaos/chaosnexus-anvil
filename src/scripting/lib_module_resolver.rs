// chaosnexus-anvil/src/scripting/lib_module_resolver.rs
//
// Restricts shared `import "lib/..."` resolution to `scripts/lib/` only.

use rhai::module_resolvers::{FileModuleResolver, ModuleResolver};
use rhai::{EvalAltResult, Position};
use std::path::PathBuf;
use std::sync::Arc;

/// Resolves `import "lib/<name>"` against `scripts/lib/` exclusively.
#[derive(Debug)]
pub struct SharedLibModuleResolver {
    inner: FileModuleResolver,
}

impl SharedLibModuleResolver {
    /// Creates a new resolver rooted at `lib_root`.
    pub fn new(lib_root: PathBuf) -> Self {
        Self {
            inner: FileModuleResolver::new_with_path(lib_root),
        }
    }
}

impl ModuleResolver for SharedLibModuleResolver {
    fn resolve(
        &self,
        engine: &rhai::Engine,
        source: Option<&str>,
        path: &str,
        pos: Position,
    ) -> Result<Arc<rhai::Module>, Box<EvalAltResult>> {
        let Some(rel) = path.strip_prefix("lib/") else {
            return Err(Box::new(EvalAltResult::ErrorModuleNotFound(
                path.into(),
                pos,
            )));
        };
        if rel.contains("..") || rel.starts_with('/') || rel.starts_with('\\') {
            return Err(format!("Module path '{path}' is not allowed.").into());
        }
        self.inner.resolve(engine, source, rel, pos)
    }
}
