// chaosnexus-anvil/src/lib.rs

//! # ChaosNexus Anvil Core Library
//!
//! ChaosNexus Anvil is a high-performance, local-first Model Context Protocol (MCP) host engine
//! and embedded Rhai scripting runtime. It exposes script-defined tools to LLM orchestrators and
//! consumes outbound stdio/HTTP MCP servers.

/// Configuration loading, cascading inheritance, and plugin permission definitions.
pub mod config;
/// The core Rhai scripting engine, capability sandboxing, execution tracing, and plugin manager.
pub mod scripting;
/// Model Context Protocol (MCP) server implementation and tool request handler.
pub mod server;
