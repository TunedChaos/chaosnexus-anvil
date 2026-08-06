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
    use std::fs;
    use std::sync::Mutex;

    /// Serialize HOME-mutating path-cascade tests so parallel cargo test workers do not clash.
    static HOME_LOCK: Mutex<()> = Mutex::new(());

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

    /// Instance override replaces host permissions for the same plugin key (later wins).
    #[test]
    fn test_instance_merge_widens_http_methods() {
        let mut global: Config = toml::from_str(
            r#"
            [plugins.demo.permissions]
            http = ["GET"]
            "#,
        )
        .unwrap();
        let instance: Config = toml::from_str(
            r#"
            [plugins.demo.permissions]
            http = ["GET", "POST"]
            "#,
        )
        .unwrap();
        global.merge(instance);
        let http = global
            .plugins
            .as_ref()
            .unwrap()
            .get("demo")
            .unwrap()
            .permissions
            .as_ref()
            .unwrap()
            .http
            .as_ref()
            .unwrap();
        assert_eq!(http, &vec!["GET".to_string(), "POST".to_string()]);
    }

    /// Omitting `permissions` in an override must not clear the global grant (shallow merge).
    #[test]
    fn test_merge_omitting_permissions_preserves_global() {
        let mut global: Config = toml::from_str(
            r#"
            [plugins.demo.permissions]
            http = ["GET"]
            "#,
        )
        .unwrap();
        let instance: Config = toml::from_str(
            r#"
            name = "instance-only"
            "#,
        )
        .unwrap();
        global.merge(instance);
        assert_eq!(global.name.as_deref(), Some("instance-only"));
        let http = global
            .plugins
            .as_ref()
            .unwrap()
            .get("demo")
            .unwrap()
            .permissions
            .as_ref()
            .unwrap()
            .http
            .as_ref()
            .unwrap();
        assert_eq!(http, &vec!["GET".to_string()]);
    }

    /// Global → CLI → instance path cascade via real files under a temp HOME.
    #[test]
    fn test_load_base_and_instance_path_cascade() {
        let _guard = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "cn_anvil_cfg_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let home = tmp.join("home");
        let anvil_global = home.join(".chaosnexus").join("anvil");
        let instance_dir = anvil_global.join("ci");
        fs::create_dir_all(&anvil_global).unwrap();
        fs::create_dir_all(&instance_dir).unwrap();

        fs::write(
            anvil_global.join("chaosnexus-anvil.toml"),
            r#"
            name = "global"
            [plugins.demo.permissions]
            http = ["GET"]
            "#,
        )
        .unwrap();

        let cli_path = tmp.join("cli.toml");
        fs::write(
            &cli_path,
            r#"
            name = "cli"
            [plugins.demo.permissions]
            http = ["GET", "HEAD"]
            "#,
        )
        .unwrap();

        fs::write(
            instance_dir.join("chaosnexus-anvil.toml"),
            r#"
            name = "instance"
            [plugins.demo.permissions]
            http = ["GET", "POST"]
            "#,
        )
        .unwrap();

        let prev_home = std::env::var_os("HOME");
        // SAFETY: serialized by HOME_LOCK; restored before unlock.
        unsafe {
            std::env::set_var("HOME", &home);
        }

        let mut cfg = Config::load_base(Some(cli_path.to_str().unwrap())).expect("load_base");
        assert_eq!(cfg.name.as_deref(), Some("cli"));
        let http_after_cli = cfg
            .plugins
            .as_ref()
            .unwrap()
            .get("demo")
            .unwrap()
            .permissions
            .as_ref()
            .unwrap()
            .http
            .as_ref()
            .unwrap();
        assert_eq!(http_after_cli, &vec!["GET".to_string(), "HEAD".to_string()]);

        cfg.load_instance_override("ci");
        assert_eq!(cfg.name.as_deref(), Some("instance"));
        let http_after_instance = cfg
            .plugins
            .as_ref()
            .unwrap()
            .get("demo")
            .unwrap()
            .permissions
            .as_ref()
            .unwrap()
            .http
            .as_ref()
            .unwrap();
        assert_eq!(
            http_after_instance,
            &vec!["GET".to_string(), "POST".to_string()]
        );

        match prev_home {
            Some(h) => unsafe { std::env::set_var("HOME", h) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(&tmp);
    }
}
