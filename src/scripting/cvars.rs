// chaosnexus-anvil/src/scripting/cvars.rs
//
// Launch-time CVar configuration (the `server.cfg` analog).
//
// CVars are declared in plugin code via `register_cvar` (default + description)
// and may be seeded per-plugin from `plugin.toml [cvars]`. This module adds a
// workspace-level override file, `cvars.toml`, placed at the scripts root (the
// directory that contains `plugins/`). It lets an operator pin any CVar value
// at launch without editing plugin source or a plugin's bundled toml, mirroring
// the classic game-server-mod workflow (AMX Mod X / SourceMod `.cfg` files) in
// the project's native, typed, git-friendly TOML format.
//
// Precedence (lowest to highest):
//   plugin.toml [cvars]  ->  register_cvar() default  ->  cvars.toml  ->  set_cvar() at runtime
//
// Because overrides are pre-seeded before plugins load, `on_plugin_start` reads
// the final overridden value.

use crate::scripting::models::CVar;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// Filename of the workspace-level launch override config, resolved relative to
/// the scripts root (the parent of `plugins/`).
pub const CVAR_CONFIG_FILENAME: &str = "cvars.toml";

/// Deserialized shape of `cvars.toml`. Unknown keys are ignored so the file can
/// carry comments and future sections without breaking older engines.
#[derive(serde::Deserialize, Default)]
struct CvarConfigIn {
    #[serde(default)]
    cvars: HashMap<String, String>,
}

/// Serializable shape used when persisting. A `BTreeMap` guarantees a stable,
/// alphabetically-sorted output for clean git diffs.
#[derive(serde::Serialize)]
struct CvarConfigOut {
    cvars: BTreeMap<String, String>,
}

/// Resolves `<scripts_root>/cvars.toml`.
pub fn config_path(scripts_root: &Path) -> PathBuf {
    scripts_root.join(CVAR_CONFIG_FILENAME)
}

/// Loads launch-time CVar overrides from `<scripts_root>/cvars.toml`.
///
/// Returns an empty map when the file is absent or unparseable, so a missing
/// or malformed config never blocks engine startup (a warning is logged).
pub fn load_overrides(scripts_root: &Path) -> HashMap<String, String> {
    let path = config_path(scripts_root);
    // Return-early when there is no launch config.
    if !path.exists() {
        return HashMap::new();
    }

    match std::fs::read_to_string(&path) {
        Ok(contents) => match toml::from_str::<CvarConfigIn>(&contents) {
            Ok(parsed) => parsed.cvars,
            Err(e) => {
                eprintln!("[chaosnexus-anvil] Failed to parse {:?}: {}", path, e);
                HashMap::new()
            }
        },
        Err(e) => {
            eprintln!("[chaosnexus-anvil] Failed to read {:?}: {}", path, e);
            HashMap::new()
        }
    }
}

/// Serializes the live CVar registry into a compact single-line JSON array,
/// sorted by name. Used by the supervised-engine IPC dump consumed by
/// ChaosNexus Forge's CVar Controller.
///
/// Shape: `[{"name","value","description","plugin"}, ...]`.
pub fn to_json(cvars: &HashMap<String, CVar>) -> String {
    let mut sorted: Vec<&CVar> = cvars.values().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    serde_json::to_string(&sorted).unwrap_or_else(|_| "[]".to_string())
}

/// Persists the current CVar values to `<scripts_root>/cvars.toml`.
///
/// All currently-registered CVars are written as a full snapshot, so the file
/// becomes an authoritative launch baseline (operators can prune entries by
/// hand). Returns a human-readable confirmation or an error string.
pub fn persist(scripts_root: &Path, cvars: &HashMap<String, CVar>) -> Result<String, String> {
    let mut map = BTreeMap::new();
    for cvar in cvars.values() {
        map.insert(cvar.name.clone(), cvar.value.clone());
    }
    let count = map.len();

    let body = toml::to_string_pretty(&CvarConfigOut { cvars: map })
        .map_err(|e| format!("Failed to serialize cvars: {}", e))?;

    let header = "# cvars.toml - ChaosNexus Anvil launch-time CVar overrides\n\
                  # Auto-managed by ChaosNexus Forge; values here override plugin defaults at launch.\n\
                  # Precedence: plugin.toml < register_cvar() default < this file < runtime set.\n\n";

    let path = config_path(scripts_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }
    std::fs::write(&path, format!("{}{}", header, body))
        .map_err(|e| format!("Failed to write {:?}: {}", path, e))?;

    Ok(format!("Saved {} CVar(s) to {:?}", count, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cvar(name: &str, value: &str) -> CVar {
        CVar {
            plugin_name: "test".to_string(),
            name: name.to_string(),
            value: value.to_string(),
            description: format!("desc for {}", name),
        }
    }

    #[test]
    fn load_overrides_missing_file_is_empty() {
        let dir = std::env::temp_dir().join("cw_cvars_missing_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(load_overrides(&dir).is_empty());
    }

    #[test]
    fn persist_then_load_roundtrips() {
        let dir = std::env::temp_dir().join("cw_cvars_roundtrip_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut map = HashMap::new();
        map.insert("sv_gravity".to_string(), cvar("sv_gravity", "800"));
        map.insert("mp_timelimit".to_string(), cvar("mp_timelimit", "30"));

        persist(&dir, &map).unwrap();
        let loaded = load_overrides(&dir);
        assert_eq!(loaded.get("sv_gravity").map(String::as_str), Some("800"));
        assert_eq!(loaded.get("mp_timelimit").map(String::as_str), Some("30"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn to_json_is_sorted_array() {
        let mut map = HashMap::new();
        map.insert("zebra".to_string(), cvar("zebra", "1"));
        map.insert("alpha".to_string(), cvar("alpha", "2"));
        let json = to_json(&map);
        let alpha_idx = json.find("alpha").unwrap();
        let zebra_idx = json.find("zebra").unwrap();
        assert!(
            alpha_idx < zebra_idx,
            "expected alpha before zebra: {}",
            json
        );
    }
}
