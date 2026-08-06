// chaosnexus-anvil/src/scripting/native_api/core.rs
use crate::scripting::capabilities::Capability;
use crate::scripting::manager::write_log;
use crate::scripting::models::CVar;
use crate::scripting::models::NativeContext;
use crate::scripting::native_api::gates::{require_cap};
use crate::scripting::plugin_context::{
    RESERVED_SCRIPT_EVENTS, capabilities_for, enter_callback, namespaced_key,
};
use crate::scripting::secrets::get_secret;
use rhai::Engine;

/// Resolves a translated phrase for the current plugin from the shared translation tree.
fn translate_phrase(
    ctx: &NativeContext,
    phrase_key: &str,
    locale: &str,
    args: &rhai::Array,
) -> String {
    let plugin_name = crate::scripting::plugin_context::current_plugin_name();
    let t = ctx.translations.read().unwrap();
    let plugin_t_opt = t.get(&plugin_name).cloned();
    drop(t); // Drop lock before string manipulation

    let Some(plugin_t) = plugin_t_opt else {
        let mut result = phrase_key.to_string();
        for (i, arg) in args.iter().enumerate() {
            let placeholder = format!("{{{}}}", i);
            result = result.replace(&placeholder, &arg.to_string());
        }
        return result;
    };

    let phrase = if let Some(locale_t) = plugin_t.get(locale)
        && let Some(p) = locale_t.get(phrase_key)
    {
        Some(p.clone())
    } else if locale != "en"
        && let Some(en_t) = plugin_t.get("en")
        && let Some(p) = en_t.get(phrase_key)
    {
        Some(p.clone())
    } else {
        None
    };

    let mut result = phrase.unwrap_or_else(|| phrase_key.to_string());

    for (i, arg) in args.iter().enumerate() {
        let placeholder = format!("{{{}}}", i);
        result = result.replace(&placeholder, &arg.to_string());
    }

    result
}

/// Registers core native functions with the Rhai engine.
pub fn register(engine: &mut Engine, n_ctx: &NativeContext) {
    engine.register_fn(
        "assert",
        |condition: bool, msg: &str| -> Result<(), Box<rhai::EvalAltResult>> {
            if !condition {
                return Err(format!("Assertion failed: {}", msg).into());
            }
            Ok(())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "create_event",
        move |name: &str| -> Result<(), Box<rhai::EvalAltResult>> {
            let mut evs = ctx.events.lock().unwrap();
            if !evs.contains_key(name) {
                evs.insert(name.to_string(), Vec::new());
            }
            Ok(())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "hook_event",
        move |event_name: &str,
              callback: &str|
              -> Result<(), Box<rhai::EvalAltResult>> {
            let plugin_name = crate::scripting::native_api::gates::verify_current_plugin(&ctx, Capability::FsCrossPlugin)?;
            let plugin_name = plugin_name.as_str();
            let mut evs = ctx.events.lock().unwrap();
            let hooks = evs.get_mut(event_name).ok_or_else(|| format!("Event {} does not exist", event_name))?;
            hooks.push((plugin_name.to_string(), callback.to_string()));
            Ok(())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "fire_event",
        move |context: rhai::NativeCallContext,
              event_name: &str,
              data: rhai::Dynamic|
              -> Result<(), Box<rhai::EvalAltResult>> {
            if RESERVED_SCRIPT_EVENTS.contains(&event_name) {
                return Err(format!(
                    "Event '{}' is reserved for the engine and cannot be fired by scripts.",
                    event_name
                )
                .into());
            }
            let _guard = enter_callback()?;
            let to_call = ctx.get_events_for(event_name);

            let mut calls_with_ast = Vec::new();
            for (p_name, cb_name) in to_call {
                if let Some(ast) = ctx.get_ast(&p_name) {
                    calls_with_ast.push((p_name, cb_name, ast));
                }
            }

            for (p_name, cb_name, ast) in calls_with_ast {
                let mut scope = rhai::Scope::new();
                scope.push("PLUGIN_NAME", p_name.clone());
                let _ = context.engine().call_fn::<rhai::Dynamic>(
                    &mut scope,
                    &ast,
                    &cb_name,
                    (data.clone(),),
                );
            }
            Ok(())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn("set_global", move |key: &str, value: rhai::Dynamic| {
        let plugin = crate::scripting::plugin_context::current_plugin()
            .unwrap_or_else(|| "unknown".to_string());
        let caps = capabilities_for(&ctx.plugin_capabilities, &plugin);
        let ns_key = namespaced_key(key, &caps);
        let mut state = ctx.global_state.write().unwrap();
        state.insert(ns_key, value);
    });

    let ctx = n_ctx.clone();
    engine.register_fn("get_global", move |key: &str| -> rhai::Dynamic {
        let plugin = crate::scripting::plugin_context::current_plugin()
            .unwrap_or_else(|| "unknown".to_string());
        let caps = capabilities_for(&ctx.plugin_capabilities, &plugin);
        let ns_key = namespaced_key(key, &caps);
        let state = ctx.global_state.read().unwrap();
        if let Some(val) = state.get(&ns_key) {
            return val.clone();
        }
        rhai::Dynamic::UNIT
    });

    engine.register_fn("log_trace", |ctx: rhai::NativeCallContext, msg: &str| {
        write_log(ctx.fn_source().unwrap_or("Rhai"), "TRACE", msg);
    });
    engine.register_fn("log_debug", |ctx: rhai::NativeCallContext, msg: &str| {
        write_log(ctx.fn_source().unwrap_or("Rhai"), "DEBUG", msg);
    });
    engine.register_fn("log_info", |ctx: rhai::NativeCallContext, msg: &str| {
        write_log(ctx.fn_source().unwrap_or("Rhai"), "INFO", msg);
    });
    engine.register_fn("log_info", |tag: &str, msg: &str| {
        write_log(tag, "INFO", msg);
    });
    engine.register_fn("log_warn", |ctx: rhai::NativeCallContext, msg: &str| {
        write_log(ctx.fn_source().unwrap_or("Rhai"), "WARN", msg);
    });
    engine.register_fn("log_warn", |tag: &str, msg: &str| {
        write_log(tag, "WARN", msg);
    });
    engine.register_fn("log_error", |ctx: rhai::NativeCallContext, msg: &str| {
        write_log(ctx.fn_source().unwrap_or("Rhai"), "ERROR", msg);
    });
    engine.register_fn("log_error", |tag: &str, msg: &str| {
        write_log(tag, "ERROR", msg);
    });



    let ctx = n_ctx.clone();
    engine.register_fn(
        "mcp_log",
        move |level: &str, msg: &str| {
            let plugin_name = crate::scripting::plugin_context::current_plugin_name();
            if let Some(tx) = &ctx.mcp_log_tx {
                let _ = tx.send((plugin_name, level.to_string(), msg.to_string()));
            }
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "translate",
        move |phrase_key: &str, locale: &str, args: rhai::Array| -> String {
            translate_phrase(&ctx, phrase_key, locale, &args)
        },
    );
    // 4-arg overload used by bundled examples: translate(plugin, key, locale, args).
    // Plugin name is identity-checked; lookup still uses CURRENT_PLUGIN translations.
    let ctx = n_ctx.clone();
    engine.register_fn(
        "translate",
        move |plugin_name: &str,
              phrase_key: &str,
              locale: &str,
              args: rhai::Array|
              -> Result<String, Box<rhai::EvalAltResult>> {
            if let Some(current) = crate::scripting::plugin_context::current_plugin()
                && current != plugin_name
            {
                let caller_caps = crate::scripting::plugin_context::capabilities_for(
                    &ctx.plugin_capabilities,
                    &current,
                );
                crate::scripting::plugin_context::verify_plugin_identity(
                    plugin_name,
                    &caller_caps,
                    crate::scripting::capabilities::Capability::FsCrossPlugin,
                )
                .map_err(|e: String| -> Box<rhai::EvalAltResult> { e.into() })?;
            }
            Ok(translate_phrase(&ctx, phrase_key, locale, &args))
        },
    );

    let _ctx = n_ctx.clone();
    engine.register_fn("sleep", move |ms: i64| {
        std::thread::sleep(std::time::Duration::from_millis(ms as u64));
    });

    let ctx = n_ctx.clone();
    engine.register_fn(
        "create_timer",
        move |ms: i64, repeat: bool, callback: &str, payload: rhai::Dynamic| {
            let plugin_name = crate::scripting::plugin_context::current_plugin_name();
            let _ = ctx.timer_tx.clone().send((
                plugin_name,
                callback.to_string(),
                ms,
                repeat,
                payload,
            ));
        },
    );
    let ctx = n_ctx.clone();
    engine.register_fn(
        "get_env",
        move |key: &str| -> Result<String, Box<rhai::EvalAltResult>> {
            get_secret(&ctx, key).map_err(|e| e.into())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "register_cvar",
        move |name: &str, default_val: &str, desc: &str| {
            let plugin_name = crate::scripting::plugin_context::current_plugin_name();
            let mut c = ctx.cvars.write().unwrap();
            let existing = c.entry(name.to_string()).or_insert_with(|| CVar {
                plugin_name: plugin_name.clone(),
                name: name.to_string(),
                value: default_val.to_string(),
                description: desc.to_string(),
            });
            if existing.description.is_empty() {
                existing.description = desc.to_string();
            }
            if existing.plugin_name.is_empty() {
                existing.plugin_name = plugin_name;
            }
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "register_cvar",
        move |plugin_name: &str, name: &str, default_val: &str, desc: &str| {
            let mut c = ctx.cvars.write().unwrap();
            let existing = c.entry(name.to_string()).or_insert_with(|| CVar {
                plugin_name: plugin_name.to_string(),
                name: name.to_string(),
                value: default_val.to_string(),
                description: desc.to_string(),
            });
            if existing.description.is_empty() {
                existing.description = desc.to_string();
            }
            if existing.plugin_name.is_empty() {
                existing.plugin_name = plugin_name.to_string();
            }
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn("get_cvar", move |name: &str| -> String {
        let c = ctx.cvars.read().unwrap();
        if let Some(val) = c.get(name) {
            return val.value.clone();
        }
        String::new()
    });

    let ctx = n_ctx.clone();
    engine.register_fn(
        "register_native",
        move |native_name: &str, target_func: &str| {
            let plugin_name = crate::scripting::plugin_context::current_plugin_name();
            let mut n = ctx.natives.write().unwrap();
            n.insert(
                native_name.to_string(),
                (plugin_name, target_func.to_string()),
            );
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "call_native",
        move |context: rhai::NativeCallContext,
              native_name: &str,
              args: rhai::Array|
              -> Result<rhai::Dynamic, Box<rhai::EvalAltResult>> {
            let _guard = enter_callback()?;
            let (p_name, f_name) = {
                let n = ctx.natives.read().unwrap();
                n.get(native_name).cloned().ok_or_else(|| format!("Native '{}' not found", native_name))?
            };

            let Some(ast_clone) = ctx.get_ast(&p_name) else {
                return Err(format!("Plugin '{}' for Native '{}' not found", p_name, native_name).into());
            };

            let mut scope = rhai::Scope::new();
            scope.push("PLUGIN_NAME", p_name.clone());
            context.engine().call_fn(&mut scope, &ast_clone, &f_name, (args,))
        },
    );
    engine.register_fn(
        "regex_match",
        |pattern: &str, text: &str| -> Result<rhai::Array, Box<rhai::EvalAltResult>> {
            let re = regex::Regex::new(pattern).map_err(|e| e.to_string())?;
            let mut arr = rhai::Array::new();
            for caps in re.captures_iter(text) {
                if let Some(m) = caps.get(0) {
                    arr.push(rhai::Dynamic::from(m.as_str().to_string()));
                }
            }
            Ok(arr)
        },
    );
    engine.register_fn(
        "regex_replace",
        |pattern: &str, text: &str, rep: &str| -> Result<String, Box<rhai::EvalAltResult>> {
            let re = regex::Regex::new(pattern).map_err(|e| e.to_string())?;
            Ok(re.replace_all(text, rep).to_string())
        },
    );
    engine.register_fn(
        "system_time",
        |_timezone: &str| -> Result<String, Box<rhai::EvalAltResult>> {
            Ok(chrono::Utc::now().to_rfc3339()) // Returns current UTC time as RFC 3339 string
        },
    );
    let ctx = n_ctx.clone();
    engine.register_fn(
        "ntp_request",
        // _port is reserved for future use; the NTP library resolves the default port internally.
        move |server: &str, _port: i64| -> Result<String, Box<rhai::EvalAltResult>> {
            require_cap(&ctx, Capability::NetHttp)?;
            let client = rsntp::SntpClient::new();
            let ntp_result = client
                .synchronize(server)
                .map_err(|e| format!("NTP error: {}", e))?;
            let sec = ntp_result
                .datetime()
                .unix_timestamp()
                .map_err(|e| e.to_string())?
                .as_secs();
            let offset_str = format!("{:?}", ntp_result.clock_offset());
            Ok(format!(
                "{{\"sec\":{}, \"offset\":\"{}\"}}",
                sec, offset_str
            ))
        },
    );
}
