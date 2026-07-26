use crate::scripting::capabilities::Capability;
use crate::scripting::models::NativeContext;
use crate::scripting::native_api::gates::{require_cap};
use crate::scripting::paths::plugins_root;
use crate::scripting::utils::*;
use rhai::Engine;
use std::sync::Arc;

#[cfg(feature = "database")]
use sea_orm::FromQueryResult;
#[cfg(feature = "database")]
use sea_orm::{ConnectionTrait, Database, Statement};

#[cfg(feature = "database")]
#[derive(Clone)]
pub struct SkytableHandle {
    pub connection: Arc<tokio::sync::Mutex<skytable::Connection>>,
}

#[cfg(feature = "database")]
#[derive(Clone)]
pub struct RedisHandle {
    pub client: Arc<redis::Client>,
}

fn verify_db_connection(ctx: &NativeContext, plugin_name: &str, url: &str) -> Result<(), Box<rhai::EvalAltResult>> {
    let perms = ctx.plugins.read().unwrap();
    let plugin_config = perms.get(plugin_name);
    
    if let Some(config) = plugin_config
        && let Some(permissions) = &config.permissions
        && let Some(db_urls) = permissions.sql_urls.as_ref()
            && db_urls.iter().any(|allowed_url| url.starts_with(allowed_url) || allowed_url == "*") {
                return Ok(());
            }
    Err(format!("Security Violation: Database connection to '{}' is not explicitly allowed in chaosnexus-anvil.toml for plugin '{}'", url, plugin_name).into())
}

fn verify_db_operation(ctx: &NativeContext, sql: &str) -> Result<(), Box<rhai::EvalAltResult>> {
    let plugin_name = crate::scripting::plugin_context::current_plugin()
        .unwrap_or_else(|| "unknown".to_string());
    
    let perms = ctx.plugins.read().unwrap();
    let plugin_config = perms.get(&plugin_name);

    let sql_upper = sql.trim_start().to_uppercase();
    let op = sql_upper.split_whitespace().next().unwrap_or("UNKNOWN");

    if let Some(config) = plugin_config
        && let Some(permissions) = &config.permissions
        && let Some(db_ops) = permissions.sql.as_ref()
            && db_ops.iter().any(|o| o.eq_ignore_ascii_case(op) || o == "*") {
                return Ok(());
            }
    
    Err(format!("Security Violation: Database operation '{}' is not explicitly allowed in chaosnexus-anvil.toml for plugin '{}'. Deny-by-default is in effect.", op, plugin_name).into())
}

/// Registers database native functions with the Rhai engine.
pub fn register(engine: &mut Engine, n_ctx: &NativeContext) {
    #[cfg(feature = "database")]
    register_database_features(engine, n_ctx);
}

#[cfg(feature = "database")]
fn register_database_features(engine: &mut Engine, n_ctx: &NativeContext) {
    let ctx = n_ctx.clone();
    
    // Original db_connect (with explicit URL)
    engine.register_fn("db_connect", move |id: &str, url: &str| -> Result<(), Box<rhai::EvalAltResult>> {
        let plugin_name = crate::scripting::native_api::gates::verify_current_plugin(&ctx, Capability::FsCrossPlugin)?;
            let plugin_name = plugin_name.as_str();
        let mut final_url = url.to_string();

        if final_url.starts_with("sqlite://") {
            let path_part = final_url.strip_prefix("sqlite://").unwrap();
            if path_part.contains("../") || path_part.starts_with("/") {
                return Err("SQLite databases must remain within the plugin's directory. Absolute paths or '../' are forbidden.".into());
            }
            let db_path = plugins_root().join(plugin_name).join(path_part);
            final_url = format!("sqlite://{}", db_path.display());
        } else {
            require_cap(&ctx, Capability::DbExternal)?;
            verify_db_connection(&ctx, plugin_name, &final_url)?;
        }

        let res = run_async(async move {
            Database::connect(&final_url).await
        });
        let conn = res.map_err(|e| format!("DB Connect error: {}", e))?;
        ctx.db_connections.lock().unwrap().insert(id.to_string(), conn);
        Ok(())
    });

    // Ambient connection (zero-credential)
    let ctx2 = n_ctx.clone();
    engine.register_fn(
        "db_connect",
        move |id: &str| -> Result<(), Box<rhai::EvalAltResult>> {
            let plugin_name = crate::scripting::plugin_context::current_plugin_name();
            let plugin_name = plugin_name.as_str();
            require_cap(&ctx2, Capability::DbExternal)?;
            let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL environment variable is not set".to_string())?;
            verify_db_connection(&ctx2, plugin_name, &url)?;
        
        let res = run_async(async move {
            Database::connect(&url).await
        });
        let conn = res.map_err(|e| format!("DB Connect error: {}", e))?;
        ctx2.db_connections.lock().unwrap().insert(id.to_string(), conn);
        Ok(())
    });

    let ctx3 = n_ctx.clone();
    engine.register_fn(
        "db_execute",
        move |id: &str, sql: &str, params: rhai::Array| -> Result<i64, Box<rhai::EvalAltResult>> {
            verify_db_operation(&ctx3, sql)?;
            let values = rhai_array_to_sea_values(params);
            let conn_guard = ctx3.db_connections.lock().unwrap();
            let conn = conn_guard
                .get(id)
                .ok_or_else(|| format!("No DB connection {}", id))?
                .clone();
            drop(conn_guard);

            let backend = conn.get_database_backend();
            let stmt = Statement::from_sql_and_values(backend, sql, values);

            let res = run_async(async move { conn.execute(stmt).await });

            let exec_res = res.map_err(|e| format!("DB Execute error: {}", e))?;
            Ok(exec_res.rows_affected() as i64)
        },
    );

    let ctx4 = n_ctx.clone();
    engine.register_fn(
        "db_query",
        move |id: &str,
              sql: &str,
              params: rhai::Array|
              -> Result<rhai::Array, Box<rhai::EvalAltResult>> {
            verify_db_operation(&ctx4, sql)?;
            let values = rhai_array_to_sea_values(params);
            let conn_guard = ctx4.db_connections.lock().unwrap();
            let conn = conn_guard
                .get(id)
                .ok_or_else(|| format!("No DB connection {}", id))?
                .clone();
            drop(conn_guard);

            let backend = conn.get_database_backend();
            let stmt = Statement::from_sql_and_values(backend, sql, values);

            let res =
                run_async(
                    async move { serde_json::Value::find_by_statement(stmt).all(&conn).await },
                );

            let json_vals = res.map_err(|e| format!("DB Query error: {}", e))?;
            let mut arr = rhai::Array::new();
            for v in json_vals {
                arr.push(json_value_to_rhai(v));
            }
            Ok(arr)
        },
    );
    
    // Skytable native driver wrapper
    engine.register_type::<SkytableHandle>();
    let ctx_sky = n_ctx.clone();
    engine.register_fn("skytable_connect", move || -> Result<SkytableHandle, Box<rhai::EvalAltResult>> {
        require_cap(&ctx_sky, Capability::DbExternal)?;
        let url = std::env::var("SKYTABLE_URL").unwrap_or_else(|_| "127.0.0.1:2003".to_string());
        
        let mut parts = url.split(':');
        let host = parts.next().unwrap_or("127.0.0.1").to_string();
        let port = parts.next().and_then(|p| p.parse::<u16>().ok()).unwrap_or(2003);
        
        let res = skytable::Connection::new(&host, port);
        
        let con = res.map_err(|e| format!("Skytable connect error: {}", e))?;
        Ok(SkytableHandle { connection: Arc::new(tokio::sync::Mutex::new(con)) })
    });
    
    // Redis native driver wrapper
    engine.register_type::<RedisHandle>();
    let ctx_redis = n_ctx.clone();
    engine.register_fn("redis_connect", move || -> Result<RedisHandle, Box<rhai::EvalAltResult>> {
        require_cap(&ctx_redis, Capability::DbExternal)?;
        let url = std::env::var("REDIS_URL").map_err(|_| "REDIS_URL environment variable is not set".to_string())?;
        
        let client = redis::Client::open(url).map_err(|e| format!("Redis connect error: {}", e))?;
        Ok(RedisHandle { client: Arc::new(client) })
    });

    #[cfg(feature = "files")]
    {
        let ctx_file = n_ctx.clone();
        engine.register_fn("db_query_file", move |file_path: &str, query: &str| -> Result<String, Box<rhai::EvalAltResult>> {
            // Check filesystem permissions
            let plugin_name = crate::scripting::plugin_context::current_plugin_name();
            let full_path = crate::scripting::native_api::fs::resolve_and_verify_fs(&ctx_file, &plugin_name, file_path, "R")
                .map_err(|e| format!("FS access denied: {}", e))?;

            use polars::prelude::*;
            use polars::sql::SQLContext;

            let mut ctx = SQLContext::new();
            let lazy_frame = LazyCsvReader::new(full_path.to_str().unwrap().into()).finish()
                .map_err(|e| format!("Failed to read CSV: {}", e))?;
            
            ctx.register("data", lazy_frame);
            let mut df = ctx.execute(query)
                .map_err(|e| format!("Failed to execute SQL: {}", e))?
                .collect()
                .map_err(|e| format!("Failed to collect DataFrame: {}", e))?;
            
            let mut buf = Vec::new();
            JsonWriter::new(&mut buf).finish(&mut df)
                .map_err(|e| format!("Failed to write JSON: {}", e))?;
            
            let json_str = String::from_utf8(buf)
                .map_err(|e| format!("Invalid UTF-8 in JSON: {}", e))?;
            
            Ok(json_str)
        });
    }
}
