// chaosnexus-anvil/src/scripting/native_api/sys.rs
use crate::scripting::capabilities::Capability;
use crate::scripting::models::NativeContext;
use crate::scripting::native_api::gates::require_cap;
use crate::scripting::scaffold::{self, PendingManifest};
use crate::scripting::shell_exec::{run_argv, run_shell, shell_c_allowed_by_env};

fn argv_looks_like_interpreter_eval(exec: &str, args: &[String]) -> bool {
    let base = std::path::Path::new(exec)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(exec)
        .to_ascii_lowercase();
    let interpreters = ["python", "python3", "node", "nodejs", "ruby", "perl", "php", "lua", "osascript"];
    if !interpreters.iter().any(|i| base == *i || base.starts_with(&format!("{i}."))) {
        return false;
    }
    args.iter().any(|a| {
        matches!(
            a.as_str(),
            "-c" | "--eval" | "-e" | "-Command" | "-EncodedCommand"
        )
    })
}


use chrono::Utc;
use rhai::Engine;

/// Registers system and OS level native functions with the Rhai engine.
pub fn register(engine: &mut Engine, n_ctx: &NativeContext) {
    let _ctx = n_ctx.clone();

    engine.register_fn("sys_os", || -> String { std::env::consts::OS.to_string() });

    // Legacy overload: run_command(shell, command_string) → shell -c …
    // Closed by default; enable only with CHAOSNEXUS_ANVIL_ALLOW_SHELL_C=1.
    let ctx = n_ctx.clone();
    engine.register_fn(
        "run_command",
        move |shell: &str, command: &str| -> Result<String, Box<rhai::EvalAltResult>> {
            require_cap(&ctx, Capability::Shell)?;

            if !shell_c_allowed_by_env() {
                return Err(
                    "Security Violation: run_command(shell, command) (-c path) is disabled. \
                     Use run_command(exec, args) with a discrete argv array, or set \
                     CHAOSNEXUS_ANVIL_ALLOW_SHELL_C=1 to re-enable the legacy shell -c path."
                        .into(),
                );
            }

            let plugin_name = crate::scripting::plugin_context::current_plugin()
                .unwrap_or_else(|| "unknown".to_string());

            let allowed_commands = {
                let perms = ctx.plugins.read().unwrap();
                perms
                    .get(&plugin_name)
                    .and_then(|config| config.permissions.as_ref())
                    .and_then(|p| p.shell.clone())
                    .unwrap_or_default()
            };

            let base_binary = command.split_whitespace().next().unwrap_or("");
            if !allowed_commands.iter().any(|c| c == base_binary) {
                return Err(format!(
                    "Security Violation: command '{}' is not in the allowed shell list for plugin '{}'",
                    base_binary, plugin_name
                )
                .into());
            }

            let output =
                run_shell(shell, command).map_err(|e| format!("Command exec failed: {}", e))?;
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if output.status.success() {
                Ok(stdout)
            } else {
                Err(format!("Command failed ({}):\n{}", output.status, stderr).into())
            }
        },
    );

    // Preferred: run_command(exec, args) — no shell -c.
    let ctx = n_ctx.clone();
    engine.register_fn(
        "run_command",
        move |exec: &str, args: rhai::Array| -> Result<String, Box<rhai::EvalAltResult>> {
            require_cap(&ctx, Capability::Shell)?;

            let plugin_name = crate::scripting::plugin_context::current_plugin()
                .unwrap_or_else(|| "unknown".to_string());

            let allowed_commands = {
                let perms = ctx.plugins.read().unwrap();
                perms
                    .get(&plugin_name)
                    .and_then(|config| config.permissions.as_ref())
                    .and_then(|p| p.shell.clone())
                    .unwrap_or_default()
            };

            let base_binary = exec.trim();
            if !allowed_commands.iter().any(|c| c == base_binary) {
                return Err(format!(
                    "Security Violation: command '{}' is not in the allowed shell list for plugin '{}'",
                    base_binary, plugin_name
                )
                .into());
            }

            let arg_strings: Vec<String> = args.into_iter().map(|a| a.to_string()).collect();
            if argv_looks_like_interpreter_eval(base_binary, &arg_strings) {
                return Err(
                    "Security Violation: interpreter -c/--eval style arguments are blocked.                      Allow a dedicated script file path instead, or enable CHAOSNEXUS_ANVIL_ALLOW_SHELL_C=1                      for the legacy shell path."
                        .into(),
                );
            }
            let output = run_argv(base_binary, &arg_strings)
                .map_err(|e| format!("Command exec failed: {}", e))?;
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if output.status.success() {
                Ok(stdout)
            } else {
                Err(format!("Command failed ({}):\n{}", output.status, stderr).into())
            }
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "cn_a_install_plugin",
        move |target_plugin_name: &str| -> Result<String, Box<rhai::EvalAltResult>> {
            require_cap(&ctx, Capability::Install)?;

            let toml_content = format!(
                "[plugin]\nname = \"{target_plugin_name}\"\ndescription = \"Fetched from registry\""
            );
            let script_content =
                format!("// Fetched {target_plugin_name}\nfn execute() {{ print(\"hello\"); }}");
            let script_name = format!("{target_plugin_name}_tool.rhai");

            let manifest = PendingManifest {
                plugin_name: target_plugin_name.to_string(),
                tool_name: format!("{target_plugin_name}_run"),
                description: "Installed by plugin via cn_a_install_plugin".to_string(),
                requested_capabilities: vec!["install".to_string()],
                created_at: Utc::now().to_rfc3339(),
            };

            scaffold::install_pending_files(
                target_plugin_name,
                &script_name,
                &toml_content,
                &script_content,
                &manifest,
                true,
            )
            .map_err(|e| format!("cn_a_install_plugin error: {e}"))?;

            Ok(format!(
                "Successfully parked {} in pending queue for user approval.",
                target_plugin_name
            ))
        },
    );

    engine.register_fn("cn_a_search_plugins", move |query: &str| -> String {
        format!("Search results for '{}':\n1. {} - Fake Plugin", query, query)
    });
}
