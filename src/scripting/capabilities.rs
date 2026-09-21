// chaosnexus-anvil/src/scripting/capabilities.rs
//
// Default-deny capability taxonomy for Rhai plugins. See docs/context/security_model.md.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Known capability identifiers enforced by native API gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Shell,
    ProcessSpawn,
    NetHttp,
    NetTcp,
    NetWs,
    HostRead,
    Env,
    DbExternal,
    Install,
    FsCrossPlugin,
    KvDump,
    SharedGlobal,
    FsSharedData,
}

impl Capability {
    /// All capability id strings (for validation and UI).
    pub fn all_ids() -> &'static [&'static str] {
        &[
            "shell",
            "process_spawn",
            "net_http",
            "net_tcp",
            "net_ws",
            "host_read",
            "env",
            "db_external",
            "install",
            "fs_cross_plugin",
            "kv_dump",
            "shared_global",
            "fs_shared_data",
        ]
    }

    /// Parses a string identifier into a `Capability` enum variant.
    pub fn from_str_id(id: &str) -> Option<Self> {
        match id {
            "shell" => Some(Self::Shell),
            "process_spawn" => Some(Self::ProcessSpawn),
            "net_http" => Some(Self::NetHttp),
            "net_tcp" => Some(Self::NetTcp),
            "net_ws" => Some(Self::NetWs),
            "host_read" => Some(Self::HostRead),
            "env" => Some(Self::Env),
            "db_external" => Some(Self::DbExternal),
            "install" => Some(Self::Install),
            "fs_cross_plugin" => Some(Self::FsCrossPlugin),
            "kv_dump" => Some(Self::KvDump),
            "shared_global" => Some(Self::SharedGlobal),
            "fs_shared_data" => Some(Self::FsSharedData),
            _ => None,
        }
    }

    /// Converts the `Capability` back to its string identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::ProcessSpawn => "process_spawn",
            Self::NetHttp => "net_http",
            Self::NetTcp => "net_tcp",
            Self::NetWs => "net_ws",
            Self::HostRead => "host_read",
            Self::Env => "env",
            Self::DbExternal => "db_external",
            Self::Install => "install",
            Self::FsCrossPlugin => "fs_cross_plugin",
            Self::KvDump => "kv_dump",
            Self::SharedGlobal => "shared_global",
            Self::FsSharedData => "fs_shared_data",
        }
    }
}

/// Granted capabilities for one plugin, parsed from `plugin.toml [capabilities]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilitySet {
    #[serde(default)]
    pub granted: HashSet<Capability>,
    /// When `env` is granted, optional allowlist of variable names (empty = deny all).
    #[serde(default)]
    pub env_allowlist: Vec<String>,
}

impl CapabilitySet {
    /// Checks whether a specific capability is granted in this set.
    pub fn has(&self, cap: Capability) -> bool {
        self.granted.contains(&cap)
    }

    /// Asserts that a capability is granted, returning an error message if it is not.
    pub fn require(&self, cap: Capability) -> Result<(), String> {
        if self.has(cap) {
            Ok(())
        } else {
            Err(format!(
                "Capability '{}' is not granted to this plugin.",
                cap.as_str()
            ))
        }
    }

    /// Checks whether access to a specific environment variable is permitted.
    pub fn env_var_allowed(&self, key: &str) -> bool {
        if !self.has(Capability::Env) {
            return false;
        }
        if self.env_allowlist.is_empty() {
            return false;
        }
        self.env_allowlist.iter().any(|k| k == key)
    }

    /// Parse capability id list from manifest strings (invalid ids ignored).
    pub fn from_id_list(ids: &[String]) -> Self {
        let mut set = Self::default();
        for id in ids {
            if let Some(cap) = Capability::from_str_id(id) {
                set.granted.insert(cap);
            }
        }
        set
    }

    /// Renders `[capabilities]` TOML section content.
    pub fn render_toml_section(&self) -> String {
        if self.granted.is_empty() && self.env_allowlist.is_empty() {
            return "[capabilities]\ngranted = []\n".to_string();
        }
        let mut ids: Vec<&str> = self.granted.iter().map(|c| c.as_str()).collect();
        ids.sort_unstable();
        let granted = ids
            .iter()
            .map(|s| format!("\"{s}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let mut out = format!("[capabilities]\ngranted = [{granted}]\n");
        if !self.env_allowlist.is_empty() {
            let vars = self
                .env_allowlist
                .iter()
                .map(|v| format!("\"{v}\""))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("env_allowlist = [{vars}]\n"));
        }
        out
    }
}

/// Parse `requested_capabilities` from MCP create_plugin (array of strings).
pub fn parse_requested_capabilities(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_deny() {
        let caps = CapabilitySet::default();
        assert!(caps.require(Capability::Shell).is_err());
    }

    #[test]
    fn from_id_list() {
        let caps = CapabilitySet::from_id_list(&["shell".to_string(), "net_http".to_string()]);
        assert!(caps.has(Capability::Shell));
        assert!(!caps.has(Capability::Install));
    }
}
