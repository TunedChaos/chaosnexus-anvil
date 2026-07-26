#[cfg(feature = "database")]
use sea_orm::sea_query::Value as SeaValue;

/// Converts a Rhai array of dynamic values into SeaORM `Value` types for SQL query binding.
#[cfg(feature = "database")]
pub fn rhai_array_to_sea_values(arr: rhai::Array) -> Vec<SeaValue> {
    arr.into_iter()
        .map(|d| {
            if d.is::<String>() {
                SeaValue::String(Some(Box::new(d.cast::<String>())))
            } else if d.is::<i64>() {
                SeaValue::BigInt(Some(d.cast::<i64>()))
            } else if d.is::<bool>() {
                SeaValue::Bool(Some(d.cast::<bool>()))
            } else if d.is::<f64>() {
                SeaValue::Double(Some(d.cast::<f64>()))
            } else {
                SeaValue::String(Some(Box::new(d.to_string())))
            }
        })
        .collect()
}

/// Recursively converts a `serde_json::Value` into a `rhai::Dynamic` for script consumption.
pub fn json_value_to_rhai(json: serde_json::Value) -> rhai::Dynamic {
    match json {
        serde_json::Value::Null => rhai::Dynamic::UNIT,
        serde_json::Value::Bool(b) => rhai::Dynamic::from(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                rhai::Dynamic::from(i)
            } else if let Some(f) = n.as_f64() {
                rhai::Dynamic::from(f)
            } else {
                rhai::Dynamic::from(n.to_string())
            }
        }
        serde_json::Value::String(s) => rhai::Dynamic::from(s),
        serde_json::Value::Array(arr) => {
            let mut rhai_arr = rhai::Array::new();
            for item in arr {
                rhai_arr.push(json_value_to_rhai(item));
            }
            rhai::Dynamic::from(rhai_arr)
        }
        serde_json::Value::Object(obj) => {
            let mut rhai_map = rhai::Map::new();
            for (k, v) in obj {
                rhai_map.insert(k.into(), json_value_to_rhai(v));
            }
            rhai::Dynamic::from(rhai_map)
        }
    }
}

/// Recursively converts a `rhai::Dynamic` value into a `serde_json::Value`.
///
/// This is the inverse of [`json_value_to_rhai`] and is used when marshalling
/// Rhai script arguments (maps/arrays) into JSON payloads for outbound MCP
/// tool calls. Unknown/opaque Rhai types fall back to their string form so a
/// call never silently drops data.
pub fn rhai_dynamic_to_json(value: rhai::Dynamic) -> serde_json::Value {
    // Return-early ordering: cheapest/most common scalar checks first.
    if value.is_unit() {
        return serde_json::Value::Null;
    }
    if value.is_bool() {
        return serde_json::Value::Bool(value.as_bool().unwrap_or(false));
    }
    if value.is_int() {
        return serde_json::Value::Number(value.as_int().unwrap_or(0).into());
    }
    if value.is_float() {
        return serde_json::Number::from_f64(value.as_float().unwrap_or(0.0))
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null);
    }
    if value.is_string() {
        return serde_json::Value::String(value.into_string().unwrap_or_default());
    }
    if value.is_array() {
        let arr = value.cast::<rhai::Array>();
        return serde_json::Value::Array(arr.into_iter().map(rhai_dynamic_to_json).collect());
    }
    if value.is_map() {
        let map = value.cast::<rhai::Map>();
        let mut obj = serde_json::Map::with_capacity(map.len());
        for (k, v) in map {
            obj.insert(k.to_string(), rhai_dynamic_to_json(v));
        }
        return serde_json::Value::Object(obj);
    }
    // Fallback: preserve the data as a string rather than dropping it.
    serde_json::Value::String(value.to_string())
}

/// Bridges synchronous Rhai callbacks to async Tokio operations by spawning
/// a blocking thread that runs the future on the current Tokio runtime handle.
pub fn run_async<F, R>(f: F) -> R
where
    F: std::future::Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    let handle = tokio::runtime::Handle::current();
    std::thread::spawn(move || handle.block_on(f))
        .join()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_to_json_round_trips_nested_structures() {
        // Build a Rhai map mirroring a realistic tool-call argument payload.
        let mut inner = rhai::Map::new();
        inner.insert("flag".into(), rhai::Dynamic::from(true));
        inner.insert("count".into(), rhai::Dynamic::from(3_i64));

        let mut map = rhai::Map::new();
        map.insert("path".into(), rhai::Dynamic::from("README.md".to_string()));
        map.insert("nested".into(), rhai::Dynamic::from(inner));
        map.insert(
            "items".into(),
            rhai::Dynamic::from(vec![
                rhai::Dynamic::from("a".to_string()),
                rhai::Dynamic::from(2_i64),
            ]),
        );

        let json = rhai_dynamic_to_json(rhai::Dynamic::from(map));

        assert_eq!(json["path"], serde_json::json!("README.md"));
        assert_eq!(json["nested"]["flag"], serde_json::json!(true));
        assert_eq!(json["nested"]["count"], serde_json::json!(3));
        assert_eq!(json["items"], serde_json::json!(["a", 2]));

        // And the inverse converter restores an equivalent structure.
        let back = json_value_to_rhai(json);
        assert!(back.is_map());
    }

    #[test]
    fn dynamic_to_json_handles_scalars_and_unit() {
        assert_eq!(
            rhai_dynamic_to_json(rhai::Dynamic::UNIT),
            serde_json::Value::Null
        );
        assert_eq!(
            rhai_dynamic_to_json(rhai::Dynamic::from(false)),
            serde_json::json!(false)
        );
        assert_eq!(
            rhai_dynamic_to_json(rhai::Dynamic::from(42_i64)),
            serde_json::json!(42)
        );
        assert_eq!(
            rhai_dynamic_to_json(rhai::Dynamic::from("hi".to_string())),
            serde_json::json!("hi")
        );
    }
}
