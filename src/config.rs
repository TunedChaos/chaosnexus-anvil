// chaosnexus-anvil/src/config.rs

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Configuration for an external MCP server to be launched and proxied.
#[derive(Debug, Deserialize, serde::Serialize, Default, Clone)]
pub struct McpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub prefix: Option<String>,
}

/// Permissions explicitly granted to a specific plugin.
#[derive(Debug, Deserialize, serde::Serialize, Default, Clone)]
pub struct Permissions {
    pub http: Option<Vec<String>>,
    pub http_domains: Option<Vec<String>>,
    pub net_allowlist: Option<Vec<String>>,
    pub sql: Option<Vec<String>>,
    pub sql_urls: Option<Vec<String>>,
    pub fs: Option<std::collections::HashMap<String, Vec<String>>>,
    pub shell: Option<Vec<String>>,
}

/// Configuration for a specific plugin.
#[derive(Debug, Deserialize, serde::Serialize, Default, Clone)]
pub struct PluginConfig {
    pub permissions: Option<Permissions>,
    pub secrets: Option<std::collections::HashMap<String, String>>,
    pub granted_capabilities: Option<Vec<String>>,
    pub env_allowlist: Option<Vec<String>>,
}

/// The root configuration structure for ChaosNexus Anvil.
#[derive(Debug, Deserialize, serde::Serialize, Default, Clone)]
pub struct Config {
    pub name: Option<String>,
    pub scripts_dir: Option<String>,
    pub mcp_servers: Option<std::collections::HashMap<String, McpServerConfig>>,
    pub max_proxy_response_length: Option<usize>,
    pub plugins: Option<std::collections::HashMap<String, PluginConfig>>,
}

impl Config {
    /// Loads the configuration using a cascading inheritance model:
    /// 1. Global config (~/.chaosnexus/anvil/chaosnexus-anvil.toml or ~/.chaosnexus/chaosnexus-anvil/config.toml)
    /// 2. Current working directory config (./chaosnexus-anvil.toml or ./config.toml)
    /// 3. CLI provided config (--config arg)
    ///
    /// Higher priority configs overwrite values from lower priority sources.
    pub fn load_base(cli_path: Option<&str>) -> anyhow::Result<Self> {
        let mut final_config = Config::default();

        if let Some(base_dirs) = directories::BaseDirs::new() {
            let home = base_dirs.home_dir();
            let candidate_paths = [
                home.join(".chaosnexus").join("anvil").join("chaosnexus-anvil.toml"),
                home.join(".chaosnexus").join("anvil").join("config.toml"),
                home.join(".chaosnexus").join("chaosnexus-anvil").join("chaosnexus-anvil.toml"),
                home.join(".chaosnexus").join("chaosnexus-anvil").join("config.toml"),
            ];
            for path in &candidate_paths {
                if path.exists()
                    && let Ok(config) = Self::load_file(path) {
                        final_config.merge(config);
                        break;
                    }
            }
        }

        // 2. CWD Config
        let cwd_candidates = [
            PathBuf::from("chaosnexus-anvil.toml"),
            PathBuf::from("config.toml"),
        ];
        for path in &cwd_candidates {
            if path.exists()
                && let Ok(config) = Self::load_file(path) {
                    final_config.merge(config);
                    break;
                }
        }

        // 3. CLI Config
        if let Some(path) = cli_path {
            let cli_config_path = PathBuf::from(path);
            if cli_config_path.exists() {
                if let Ok(config) = Self::load_file(&cli_config_path) {
                    final_config.merge(config);
                }
            } else {
                eprintln!("[warn] CLI config file not found: {}", path);
            }
        }

        Ok(final_config)
    }

    /// Loads instance-specific configuration overrides for the given instance name.
    pub fn load_instance_override(&mut self, instance_name: &str) {
        if let Some(base_dirs) = directories::BaseDirs::new() {
            let home = base_dirs.home_dir();
            let candidate_paths = [
                home.join(".chaosnexus").join("anvil").join(instance_name).join("chaosnexus-anvil.toml"),
                home.join(".chaosnexus").join("anvil").join(instance_name).join("config.toml"),
                home.join(".chaosnexus").join("chaosnexus-anvil").join(instance_name).join("chaosnexus-anvil.toml"),
                home.join(".chaosnexus").join("chaosnexus-anvil").join(instance_name).join("config.toml"),
            ];
            for path in &candidate_paths {
                if path.exists()
                    && let Ok(override_config) = Self::load_file(path) {
                        self.merge(override_config);
                        break;
                    }
            }
        }
    }

    pub fn load_file(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    /// Shallow merge: values from `other` override values in `self`.
    pub fn merge(&mut self, other: Config) {
        if other.name.is_some() {
            self.name = other.name;
        }
        if other.scripts_dir.is_some() {
            self.scripts_dir = other.scripts_dir;
        }
        if let Some(other_mcp) = other.mcp_servers {
            if let Some(self_mcp) = &mut self.mcp_servers {
                self_mcp.extend(other_mcp);
            } else {
                self.mcp_servers = Some(other_mcp);
            }
        }
        if other.max_proxy_response_length.is_some() {
            self.max_proxy_response_length = other.max_proxy_response_length;
        }
        if let Some(other_plugins) = other.plugins {
            if let Some(self_plugins) = &mut self.plugins {
                for (k, v) in other_plugins {
                    if let Some(existing) = self_plugins.get_mut(&k) {
                        if v.permissions.is_some() { existing.permissions = v.permissions; }
                        if v.secrets.is_some() { existing.secrets = v.secrets; }
                        if v.granted_capabilities.is_some() { existing.granted_capabilities = v.granted_capabilities; }
                        if v.env_allowlist.is_some() { existing.env_allowlist = v.env_allowlist; }
                    } else {
                        self_plugins.insert(k, v);
                    }
                }
            } else {
                self.plugins = Some(other_plugins);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_merging_overrides_permissions() {
        let base_toml = r#"
            [plugins.my_plugin.permissions]
            http = ["GET"]
        "#;
        let mut base: Config = toml::from_str(base_toml).unwrap();

        let override_toml = r#"
            [plugins.my_plugin.permissions]
            http = ["GET", "POST"]
        "#;
        let override_cfg: Config = toml::from_str(override_toml).unwrap();

        base.merge(override_cfg);

        let plugin_perms = base.plugins.unwrap().get("my_plugin").unwrap().permissions.clone().unwrap();
        assert_eq!(plugin_perms.http.unwrap(), vec!["GET".to_string(), "POST".to_string()]);
    }

    #[test]
    fn test_unconfigured_plugins_have_no_permissions() {
        let toml_str = r#"
            [plugins.allowed_plugin.permissions]
            shell = ["ls"]
        "#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        let plugins = cfg.plugins.unwrap();

        assert!(plugins.contains_key("allowed_plugin"));
        assert!(!plugins.contains_key("unauthorized_plugin"));
    }
}
