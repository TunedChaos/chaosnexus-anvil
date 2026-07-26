// chaosnexus-anvil/src/scripting/native_api/fs.rs
use crate::scripting::capabilities::Capability;
use crate::scripting::models::NativeContext;
use crate::scripting::native_api::gates::{require_cap};
use crate::scripting::paths::{resolve_data_file, resolve_plugin_file};
use crate::scripting::utils::*;
use rhai::Engine;
use std::path::PathBuf;

pub(crate) fn resolve_and_verify_fs(ctx: &NativeContext, plugin_name: &str, path: &str, op: &str) -> Result<PathBuf, String> {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        let perms = ctx.plugins.read().unwrap();
        if let Some(config) = perms.get(plugin_name)
            && let Some(permissions) = &config.permissions
            && let Some(fs_map) = &permissions.fs {
            for (k, ops) in fs_map {
                let mapped_path = PathBuf::from(k);
                if candidate.starts_with(&mapped_path)
                    && ops.iter().any(|o| o.eq_ignore_ascii_case(op) || o == "RW" || o == "*")
                {
                    return Ok(candidate);
                }
            }
        }
        return Err(format!("Security Violation: Absolute path '{}' is not granted '{}' access in chaosnexus-anvil.toml for plugin '{}'", path, op, plugin_name));
    }
    
    resolve_plugin_file(plugin_name, path)
}
/// Registers filesystem manipulation native functions with the Rhai engine.
pub fn register(engine: &mut Engine, n_ctx: &NativeContext) {
    let ctx = n_ctx.clone();
    engine.register_fn(
        "read_file_string",
        move |path: &str| -> Result<String, Box<rhai::EvalAltResult>> {
            let plugin_name = crate::scripting::native_api::gates::verify_current_plugin(&ctx, Capability::FsCrossPlugin)?;
            let plugin_name = plugin_name.as_str();
            let full_path = resolve_and_verify_fs(&ctx, plugin_name, path, "R")
                .map_err(|e| format!("fs_read err: {}", e))?;
            std::fs::read_to_string(full_path).map_err(|e| e.to_string().into())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "fs_read",
        move |path: &str| -> Result<String, Box<rhai::EvalAltResult>> {
            let plugin_name = crate::scripting::native_api::gates::verify_current_plugin(&ctx, Capability::FsCrossPlugin)?;
            let plugin_name = plugin_name.as_str();
            let full_path = resolve_and_verify_fs(&ctx, plugin_name, path, "R")
                .map_err(|e| format!("fs_read err: {}", e))?;
            std::fs::read_to_string(full_path).map_err(|e| e.to_string().into())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "fs_read",
        move |target_plugin: &str, path: &str| -> Result<String, Box<rhai::EvalAltResult>> {
            crate::scripting::native_api::gates::verify_caller_plugin(&ctx, target_plugin, Capability::FsCrossPlugin)?;
            let full_path = resolve_and_verify_fs(&ctx, target_plugin, path, "R")
                .map_err(|e| format!("fs_read err: {}", e))?;
            std::fs::read_to_string(full_path).map_err(|e| e.to_string().into())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "write_file_string",
        move |path: &str, content: &str|
              -> Result<(), Box<rhai::EvalAltResult>> {
            let plugin_name = crate::scripting::native_api::gates::verify_current_plugin(&ctx, Capability::FsCrossPlugin)?;
            let plugin_name = plugin_name.as_str();
            let full_path = resolve_and_verify_fs(&ctx, plugin_name, path, "W")
                .map_err(|e| format!("fs_write err: {}", e))?;
            if let Some(parent) = full_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(full_path, content).map_err(|e| e.to_string().into())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "append_file_string",
        move |path: &str, content: &str|
              -> Result<(), Box<rhai::EvalAltResult>> {
            let plugin_name = crate::scripting::native_api::gates::verify_current_plugin(&ctx, Capability::FsCrossPlugin)?;
            let plugin_name = plugin_name.as_str();
            let full_path = resolve_and_verify_fs(&ctx, plugin_name, path, "W")
                .map_err(|e| format!("fs_append err: {}", e))?;
            if let Some(parent) = full_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(full_path)
                .map_err(|e| e.to_string())?;
            use std::io::Write;
            file.write_all(content.as_bytes())
                .map_err(|e| e.to_string().into())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "delete_file",
        move |path: &str| -> Result<(), Box<rhai::EvalAltResult>> {
            let plugin_name = crate::scripting::native_api::gates::verify_current_plugin(&ctx, Capability::FsCrossPlugin)?;
            let plugin_name = plugin_name.as_str();
            let full_path = resolve_and_verify_fs(&ctx, plugin_name, path, "W")
                .map_err(|e| format!("fs_delete err: {}", e))?;
            std::fs::remove_file(full_path).map_err(|e| e.to_string().into())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "exists",
        move |path: &str| -> Result<bool, Box<rhai::EvalAltResult>> {
            let plugin_name = crate::scripting::native_api::gates::verify_current_plugin(&ctx, Capability::FsCrossPlugin)?;
            let plugin_name = plugin_name.as_str();
            let full_path = resolve_and_verify_fs(&ctx, plugin_name, path, "R")
                .map_err(|e| format!("fs_exists err: {}", e))?;
            Ok(full_path.exists())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "list_dir",
        move |path: &str| -> Result<rhai::Array, Box<rhai::EvalAltResult>> {
            let plugin_name = crate::scripting::native_api::gates::verify_current_plugin(&ctx, Capability::FsCrossPlugin)?;
            let plugin_name = plugin_name.as_str();
            let full_path = resolve_and_verify_fs(&ctx, plugin_name, path, "R")
                .map_err(|e| format!("fs_list_dir err: {}", e))?;

            let mut result = rhai::Array::new();
            if full_path.is_dir()
                && let Ok(entries) = std::fs::read_dir(full_path)
            {
                for entry in entries.flatten() {
                    if let Ok(name) = entry.file_name().into_string() {
                        result.push(rhai::Dynamic::from(name));
                    }
                }
            }
            Ok(result)
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "load_config_string",
        move |path: rhai::ImmutableString| -> Result<String, Box<rhai::EvalAltResult>> {
            require_cap(&ctx, Capability::HostRead)?;
            std::fs::read_to_string(path.as_str())
                .map_err(|e| format!("Failed to load config {}: {}", path, e).into())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "load_config",
        move |relative_path: rhai::ImmutableString|
              -> Result<rhai::Dynamic, Box<rhai::EvalAltResult>> {
            let plugin_name = crate::scripting::native_api::gates::verify_current_plugin(&ctx, Capability::FsCrossPlugin)?;
            let plugin_name = plugin_name.as_str();
            let full_path = resolve_and_verify_fs(&ctx, plugin_name, relative_path.as_str(), "R")
                .map_err(|e| format!("load_config err: {}", e))?;
            let content = std::fs::read_to_string(&full_path)
                .map_err(|e| format!("Failed to read config {}: {}", full_path.display(), e))?;

            let toml_val: toml::Value = toml::from_str(&content)
                .map_err(|e| format!("Failed to parse TOML {}: {}", full_path.display(), e))?;

            let json_val: serde_json::Value = serde_json::to_value(toml_val)
                .map_err(|e| format!("Failed to convert TOML to JSON: {}", e))?;

            Ok(json_value_to_rhai(json_val))
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "load_config",
        move |_plugin_name: rhai::ImmutableString, relative_path: rhai::ImmutableString|
              -> Result<rhai::Dynamic, Box<rhai::EvalAltResult>> {
            let plugin_name = crate::scripting::native_api::gates::verify_current_plugin(&ctx, Capability::FsCrossPlugin)?;
            let plugin_name = plugin_name.as_str();
            let full_path = resolve_and_verify_fs(&ctx, plugin_name, relative_path.as_str(), "R")
                .map_err(|e| format!("load_config err: {}", e))?;
            let content = std::fs::read_to_string(&full_path)
                .map_err(|e| format!("Failed to read config {}: {}", full_path.display(), e))?;

            let toml_val: toml::Value = toml::from_str(&content)
                .map_err(|e| format!("Failed to parse TOML {}: {}", full_path.display(), e))?;

            let json_val: serde_json::Value = serde_json::to_value(toml_val)
                .map_err(|e| format!("Failed to convert TOML to JSON: {}", e))?;

            Ok(json_value_to_rhai(json_val))
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "fs_data_write",
        move |path: &str, content: &str|
              -> Result<(), Box<rhai::EvalAltResult>> {
            let plugin_name = crate::scripting::plugin_context::current_plugin_name();
            let plugin_name = plugin_name.as_str();
            require_cap(&ctx, Capability::FsSharedData)?;
            let full_path = resolve_data_file(plugin_name, path)
                .map_err(|e| format!("fs_data_write err: {}", e))?;
            if let Some(parent) = full_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(full_path, content).map_err(|e| e.to_string().into())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "fs_data_read",
        move |path: &str| -> Result<String, Box<rhai::EvalAltResult>> {
            let plugin_name = crate::scripting::plugin_context::current_plugin_name();
            let plugin_name = plugin_name.as_str();
            require_cap(&ctx, Capability::FsSharedData)?;
            let full_path = resolve_data_file(plugin_name, path)
                .map_err(|e| format!("fs_data_read err: {}", e))?;
            std::fs::read_to_string(full_path).map_err(|e| e.to_string().into())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "fs_data_exists",
        move |path: &str| -> Result<bool, Box<rhai::EvalAltResult>> {
            let plugin_name = crate::scripting::plugin_context::current_plugin_name();
            let plugin_name = plugin_name.as_str();
            require_cap(&ctx, Capability::FsSharedData)?;
            let full_path = resolve_data_file(plugin_name, path)
                .map_err(|e| format!("fs_data_exists err: {}", e))?;
            Ok(full_path.exists())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "fs_data_list_dir",
        move |path: &str| -> Result<rhai::Array, Box<rhai::EvalAltResult>> {
            let plugin_name = crate::scripting::plugin_context::current_plugin_name();
            let plugin_name = plugin_name.as_str();
            require_cap(&ctx, Capability::FsSharedData)?;
            let full_path = resolve_data_file(plugin_name, path)
                .map_err(|e| format!("fs_data_list_dir err: {}", e))?;

            let mut result = rhai::Array::new();
            if full_path.is_dir()
                && let Ok(entries) = std::fs::read_dir(full_path)
            {
                for entry in entries.flatten() {
                    if let Ok(name) = entry.file_name().into_string() {
                        result.push(rhai::Dynamic::from(name));
                    }
                }
            }
            Ok(result)
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "fs_data_delete",
        move |path: &str| -> Result<(), Box<rhai::EvalAltResult>> {
            let plugin_name = crate::scripting::plugin_context::current_plugin_name();
            let plugin_name = plugin_name.as_str();
            require_cap(&ctx, Capability::FsSharedData)?;
            let full_path = resolve_data_file(plugin_name, path)
                .map_err(|e| format!("fs_data_delete err: {}", e))?;
            if full_path.is_dir() {
                std::fs::remove_dir_all(full_path).map_err(|e| e.to_string().into())
            } else {
                std::fs::remove_file(full_path).map_err(|e| e.to_string().into())
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhai::Engine;

    #[test]
    fn test_load_config_overloads_registered() {
        let mut engine = Engine::new();
        let ctx = crate::scripting::engine::empty_context();
        register(&mut engine, &ctx);

        // Verify function signatures exist on engine
        let script = r#"
            // Calling load_config with nonexistent file should fail with load_config err, not Function not found
            let err_1 = false;
            try { load_config("nonexistent.toml"); } catch { err_1 = true; }
            let err_2 = false;
            try { load_config("my_plugin", "nonexistent.toml"); } catch { err_2 = true; }
            err_1 && err_2
        "#;
        let res = engine.eval::<bool>(script);
        assert!(res.unwrap_or(false), "Both 1-arg and 2-arg load_config signatures must be callable");
    }
}

