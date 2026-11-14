use crate::scripting::models::NativeContext;
use rhai::Engine;

/// Registers JSON manipulation native functions with the Rhai engine.
pub fn register(engine: &mut Engine, _n_ctx: &NativeContext) {
    engine.register_fn(
        "json_extract",
        |json_str: &str, pointer: &str| -> Result<String, Box<rhai::EvalAltResult>> {
            let val: serde_json::Value =
                serde_json::from_str(json_str).map_err(|e| e.to_string())?;
            if let Some(v) = val.pointer(pointer) {
                if let Some(s) = v.as_str() {
                    return Ok(s.to_string());
                }
                return Ok(v.to_string());
            }
            Err("Pointer not found".into())
        },
    );
    engine.register_fn(
        "from_json",
        |json_str: &str| -> Result<rhai::Dynamic, Box<rhai::EvalAltResult>> {
            let val: serde_json::Value =
                serde_json::from_str(json_str).map_err(|e| e.to_string())?;
            rhai::serde::to_dynamic(val).map_err(|e| e.to_string().into())
        },
    );
    engine.register_fn(
        "to_json",
        |obj: rhai::Dynamic| -> Result<String, Box<rhai::EvalAltResult>> {
            let val: serde_json::Value =
                rhai::serde::from_dynamic(&obj).map_err(|e| e.to_string())?;
            serde_json::to_string(&val).map_err(|e| e.to_string().into())
        },
    );
}
