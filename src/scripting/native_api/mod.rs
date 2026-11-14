use crate::scripting::models::NativeContext;
use rhai::Engine;

pub mod core;
pub mod crypto;
#[cfg(feature = "database")]
pub mod db;
pub mod fs;
pub mod gates;
pub mod http;
pub mod json;
pub mod mcp;
pub mod mcp_client;
pub mod sys;

/// Registers all native API modules with the Rhai engine.
pub fn register_all(engine: &mut Engine, context: &NativeContext) {
    core::register(engine, context);
    fs::register(engine, context);
    http::register(engine, context);
    #[cfg(feature = "database")]
    db::register(engine, context);
    crypto::register(engine, context);
    json::register(engine, context);
    mcp::register(engine, context);
    mcp_client::register(engine, context);
    sys::register(engine, context);
    kv::register(engine, context);
}
pub mod kv;
