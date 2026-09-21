// chaosnexus-anvil/src/scripting/paths.rs
//
// Canonical filesystem roots for Rhai scripts. Initialized once when
// `PluginManager::new` runs so native APIs, module resolvers, and reload logic
// all agree on where plugins and shared libraries live.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

static SCRIPTS_ROOT: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Records the scripts root directory (the parent of `plugins/` and `lib/`).
///
/// Safe to call multiple times; only the first call wins so reload paths stay
/// stable for the process lifetime.
pub fn init(scripts_root: impl AsRef<Path>) {
    let root = scripts_root.as_ref().to_path_buf();
    if let Ok(mut guard) = SCRIPTS_ROOT.write() {
        *guard = Some(root.clone());
    }
    
    // Auto-create essential subdirectories
    let _ = std::fs::create_dir_all(root.join("plugins"));
    let _ = std::fs::create_dir_all(root.join("lib"));
    let _ = std::fs::create_dir_all(root.join(".pending"));
    let _ = std::fs::create_dir_all(root.join("data"));
}

/// Active scripts root, or `../chaosnexus-scripts` when no manager has initialized paths yet
/// (schema generation and lightweight unit tests).
pub fn scripts_root() -> PathBuf {
    SCRIPTS_ROOT
        .read()
        .ok()
        .and_then(|guard| guard.clone())
        .unwrap_or_else(|| PathBuf::from("../chaosnexus-scripts"))
}

/// `scripts/plugins`: one subdirectory per plugin.
pub fn plugins_root() -> PathBuf {
    scripts_root().join("plugins")
}

/// `scripts/lib`: shared, side-effect-free Rhai library modules.
pub fn lib_root() -> PathBuf {
    scripts_root().join("lib")
}

/// `scripts/.pending`: quarantined plugins awaiting human approval (never loaded).
pub fn pending_root() -> PathBuf {
    scripts_root().join(".pending")
}

/// `scripts/plugins/disabled`: plugins moved out of discovery (never loaded).
pub fn disabled_plugins_root() -> PathBuf {
    plugins_root().join("disabled")
}

/// Resolves a path relative to a single plugin directory with strict canonical traversal guards.
pub fn resolve_plugin_file(plugin_name: &str, relative_path: &str) -> Result<PathBuf, String> {
    if plugin_name.contains("../") || plugin_name.starts_with('/') {
        return Err("Invalid plugin name.".into());
    }
    let sandbox_root = plugins_root().join(plugin_name);
    secure_resolve(&sandbox_root, relative_path)
}

/// `scripts/data`: shared data directory with explicitly namespaced access.
pub fn data_root() -> PathBuf {
    scripts_root().join("data")
}

/// Resolves a path inside `scripts/data/<plugin_namespace>/` with strict canonical traversal guards.
pub fn resolve_data_file(plugin_namespace: &str, relative_path: &str) -> Result<PathBuf, String> {
    if plugin_namespace.contains("../") || plugin_namespace.starts_with('/') {
        return Err("Invalid plugin namespace.".into());
    }
    let sandbox_root = data_root().join(plugin_namespace);
    secure_resolve(&sandbox_root, relative_path)
}

/// Performs a highly strict sandboxed path resolution.
/// It verifies that the requested relative path, when resolved against the sandbox root,
/// never escapes the canonicalized bounds of the sandbox.
/// It gracefully handles non-existent paths (e.g. for creating new files) by canonicalizing
/// the longest existing ancestor and rejecting any ".." components in the non-existent remainder.
fn secure_resolve(sandbox_root: &Path, relative_path: &str) -> Result<PathBuf, String> {
    // Ensure the sandbox root exists so we can canonicalize it.
    // If it fails to create, it's a structural issue, but for read-only sandboxes,
    // we might just want to canonicalize if it exists. But data/plugin dirs should exist.
    let _ = std::fs::create_dir_all(sandbox_root);
    
    let canonical_sandbox = sandbox_root
        .canonicalize()
        .map_err(|_| "Access Denied: Sandbox root resolution failed".to_string())?;

    let target_path = canonical_sandbox.join(relative_path);

    let mut existing_ancestor = target_path.as_path();
    let mut non_existent_components = Vec::new();

    while !existing_ancestor.exists() {
        if let Some(parent) = existing_ancestor.parent() {
            if let Some(file_name) = existing_ancestor.file_name() {
                non_existent_components.push(file_name.to_os_string());
            } else {
                return Err("Access Denied: Invalid path structure".into());
            }
            existing_ancestor = parent;
        } else {
            return Err("Access Denied: Invalid path structure".into());
        }
    }

    let canonical_ancestor = existing_ancestor
        .canonicalize()
        .map_err(|_| "Access Denied: Path resolution failed".to_string())?;

    if !canonical_ancestor.starts_with(&canonical_sandbox) {
        return Err("Security Violation: Attempted directory escape".into());
    }

    let mut final_path = canonical_ancestor;
    for component in non_existent_components.into_iter().rev() {
        if component == ".." || component == "." {
            return Err("Security Violation: Invalid path traversal in non-existent components".into());
        }
        final_path.push(component);
    }

    if !final_path.starts_with(&canonical_sandbox) {
        return Err("Security Violation: Attempted directory escape".into());
    }

    Ok(final_path)
}
