// chaosnexus-anvil/src/scripting/native_api/mcp_client.rs
//
// Outbound MCP client bridge: exposes native Rhai functions that let plugin
// scripts launch and consume *external* MCP servers (filesystem, git, search,
// etc.). This is the "out" half of ChaosNexus Anvil's bidirectional MCP bridge; the
// "in" half (exposing our own tools to orchestrators) lives in `mcp.rs` and
// `src/server.rs`.
//
// Design notes:
// - Connections are launched over the stdio transport (the SDK spawns the child
//   process) and stored live in `NativeContext.mcp_clients`, keyed by a
//   script-provided connection id. This mirrors the `db_connect` convention.
// - Rhai executes synchronously on `spawn_blocking` workers, so every async SDK
//   call is driven through `run_async`, the established thread-bridge.
// - Results are serialized to JSON and converted to native Rhai values so
//   scripts get structured maps/arrays rather than opaque handles.

use rhai::{Engine, Module};
use rust_mcp_sdk::mcp_client::{ClientHandler, McpClientOptions, client_runtime::create_client};
use rust_mcp_sdk::schema::{
    CallToolRequestParams, ClientCapabilities, Implementation, InitializeRequestParams,
    ProtocolVersion, ReadResourceRequestParams,
};
use rust_mcp_transport::{StdioTransport, TransportOptions};

use rust_mcp_sdk::{McpClient, ToMcpClientHandler};

use crate::scripting::capabilities::Capability;
use crate::scripting::manager::write_log;
use crate::scripting::models::NativeContext;
use crate::scripting::native_api::gates::require_cap;
use crate::scripting::trace::{
    RECURSION_LIMIT_ERROR, TraceKind, build_outbound_meta, make_span, push_active_trace,
    record_span, resolve_outbound_trace, take_node_label,
};
use crate::scripting::utils::{json_value_to_rhai, rhai_dynamic_to_json, run_async};
use std::time::Instant;

/// Convenience alias for the fallible Rhai return used by every binding.
type RhaiResult<T> = Result<T, Box<rhai::EvalAltResult>>;

/// Minimal client-side handler. We do not need to react to server-initiated
/// requests (sampling, roots, elicitation) for outbound tool/resource calls,
/// so every method uses the SDK's default implementation.
struct ChaosClientHandler;

#[async_trait::async_trait]
impl ClientHandler for ChaosClientHandler {}

/// Builds the `InitializeRequestParams` advertised to downstream servers.
fn client_details() -> InitializeRequestParams {
    InitializeRequestParams {
        capabilities: ClientCapabilities::default(),
        client_info: Implementation {
            name: "chaosnexus-anvil".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            title: Some("ChaosNexus Anvil".to_string()),
            description: Some("ChaosNexus Anvil outbound MCP client".to_string()),
            icons: vec![],
            website_url: None,
        },
        protocol_version: ProtocolVersion::V2025_11_25.into(),
        meta: None,
    }
}

// --- Core operations -------------------------------------------------------
//
// The bridge logic lives in these free helpers so the flat `mcp_*` natives and
// the namespaced `mcp::` module aliases share a single implementation (DRY).

/// Launches + handshakes an external MCP server, storing the live client.
fn do_connect(
    ctx: &NativeContext,
    plugin_name: &str,
    conn_id: &str,
    command: &str,
    args: rhai::Array,
) -> RhaiResult<()> {
    require_cap(ctx, Capability::ProcessSpawn)?;
    // Return-early on collision so we never silently drop an existing
    // connection (and its child process).
    if ctx.mcp_clients.lock().unwrap().contains_key(conn_id) {
        return Err(format!("MCP client '{}' is already connected.", conn_id).into());
    }

    let command_owned = command.to_string();
    let arg_strings: Vec<String> = args.into_iter().map(|d| d.to_string()).collect();

    let result = run_async(async move {
        let transport = StdioTransport::create_with_server_launch(
            command_owned,
            arg_strings,
            None,
            TransportOptions::default(),
        )
        .map_err(|e| format!("transport launch error: {}", e))?;

        let client = create_client(McpClientOptions {
            client_details: client_details(),
            transport,
            handler: ChaosClientHandler.to_mcp_client_handler(),
            task_store: None,
            server_task_store: None,
            message_observer: None,
        });

        client
            .clone()
            .start()
            .await
            .map_err(|e| format!("client start error: {}", e))?;

        Ok::<_, String>(client)
    });

    let client = result.map_err(|e| format!("mcp_connect error: {}", e))?;
    ctx.mcp_clients
        .lock()
        .unwrap()
        .insert(conn_id.to_string(), client);
    write_log(
        plugin_name,
        "INFO",
        &format!(
            "Connected to downstream MCP server '{}' via '{}'",
            conn_id, command
        ),
    );
    Ok(())
}

/// Invokes a tool on a connected downstream server, returning a Rhai value.
fn do_call_tool(
    ctx: &NativeContext,
    conn_id: &str,
    tool_name: &str,
    mut args: rhai::Map,
) -> RhaiResult<rhai::Dynamic> {
    let node_label = take_node_label(&mut args);

    let active_snapshot = ctx.active_trace.lock().unwrap().clone();
    let next_hop = active_snapshot.as_ref().map(|a| a.hop + 1).unwrap_or(1);
    if next_hop > crate::scripting::trace::MAX_HOP_COUNT {
        let (trace_id, span_id, parent_span_id) = resolve_outbound_trace(&active_snapshot);
        let started_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        record_span(
            &ctx.trace_store,
            make_span(
                trace_id,
                span_id,
                parent_span_id,
                tool_name.to_string(),
                Some(conn_id.to_string()),
                next_hop,
                started_at_ms,
                Instant::now(),
                Some(RECURSION_LIMIT_ERROR.to_string()),
                node_label,
                TraceKind::Outbound,
            ),
        );
        return Err(format!("mcp_call_tool error: {}", RECURSION_LIMIT_ERROR).into());
    }

    let client = ctx
        .mcp_clients
        .lock()
        .unwrap()
        .get(conn_id)
        .cloned()
        .ok_or_else(|| format!("No MCP client connection '{}'", conn_id))?;

    let mut arguments = serde_json::Map::with_capacity(args.len());
    for (k, v) in args {
        arguments.insert(k.to_string(), rhai_dynamic_to_json(v));
    }

    let (trace_id, span_id, parent_span_id) = resolve_outbound_trace(&active_snapshot);
    let outbound_meta =
        build_outbound_meta(next_hop, &trace_id, &span_id, parent_span_id.as_deref());

    let params = CallToolRequestParams {
        arguments: if arguments.is_empty() {
            None
        } else {
            Some(arguments)
        },
        meta: Some(outbound_meta),
        name: tool_name.to_string(),
        task: None,
    };

    let started_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let timer = Instant::now();

    let active = crate::scripting::trace::ActiveTrace {
        trace_id: trace_id.clone(),
        hop: next_hop,
        parent_span_id: parent_span_id.clone(),
        current_span_id: span_id.clone(),
    };
    let _trace_guard = push_active_trace(&ctx.active_trace, active);

    let res = run_async(async move { client.request_tool_call(params).await });

    let error_msg = res.as_ref().err().map(|e| e.to_string());
    record_span(
        &ctx.trace_store,
        make_span(
            trace_id,
            span_id,
            parent_span_id,
            tool_name.to_string(),
            Some(conn_id.to_string()),
            next_hop,
            started_at_ms,
            timer,
            error_msg,
            node_label,
            TraceKind::Outbound,
        ),
    );

    let result = res.map_err(|e| format!("mcp_call_tool error: {}", e))?;
    let json = serde_json::to_value(&result)
        .map_err(|e| format!("result serialize error: {}", e))?;
    Ok(json_value_to_rhai(json))
}

/// Reads a resource, returning text contents (or JSON for non-text payloads).
fn do_read_resource(ctx: &NativeContext, conn_id: &str, uri: &str) -> RhaiResult<String> {
    let client = ctx
        .mcp_clients
        .lock()
        .unwrap()
        .get(conn_id)
        .cloned()
        .ok_or_else(|| format!("No MCP client connection '{}'", conn_id))?;

    let params = ReadResourceRequestParams {
        meta: None,
        uri: uri.to_string(),
    };

    let res = run_async(async move { client.request_resource_read(params).await });

    let result = res.map_err(|e| format!("mcp_read_resource error: {}", e))?;
    let mut out = String::new();
    for content in &result.contents {
        if let rust_mcp_sdk::schema::ReadResourceContent::TextResourceContents(t) = content
        {
            out.push_str(&t.text);
        }
    }
    if out.is_empty() {
        // Non-text (blob) payload: hand back the structured JSON.
        out = serde_json::to_string(&result)
            .map_err(|e| format!("result serialize error: {}", e))?;
    }
    Ok(out)
}

/// Discovers the tools a downstream server exposes.
fn do_list_tools(ctx: &NativeContext, conn_id: &str) -> RhaiResult<rhai::Dynamic> {
    let client = ctx
        .mcp_clients
        .lock()
        .unwrap()
        .get(conn_id)
        .cloned()
        .ok_or_else(|| format!("No MCP client connection '{}'", conn_id))?;

    let res = run_async(async move { client.request_tool_list(None).await });

    let result = res.map_err(|e| format!("mcp_list_tools error: {}", e))?;
    let json = serde_json::to_value(&result.tools)
        .map_err(|e| format!("tool list serialize error: {}", e))?;
    Ok(json_value_to_rhai(json))
}

/// Gracefully shuts down and drops a connection.
fn do_disconnect(ctx: &NativeContext, conn_id: &str) -> RhaiResult<()> {
    let client = ctx.mcp_clients.lock().unwrap().remove(conn_id);
    let client = client.ok_or_else(|| format!("No MCP client connection '{}'", conn_id))?;
    let _ = run_async(async move { client.shut_down().await });
    Ok(())
}

/// Registers MCP client native functions with the Rhai engine.
pub fn register(engine: &mut Engine, n_ctx: &NativeContext) {
    // Flat natives (`mcp_connect`, ...): the established, doc-generated surface.
    let ctx = n_ctx.clone();
    engine.register_fn(
        "mcp_connect",
        move |conn_id: &str, command: &str, args: rhai::Array| {
            let plugin_name = crate::scripting::plugin_context::current_plugin_name();
            do_connect(&ctx, &plugin_name, conn_id, command, args)
        },
    );
    let ctx = n_ctx.clone();
    engine.register_fn(
        "mcp_connect",
        move |plugin_name: &str, conn_id: &str, command: &str, args: rhai::Array| {
            do_connect(&ctx, plugin_name, conn_id, command, args)
        },
    );
    let ctx = n_ctx.clone();
    engine.register_fn(
        "mcp_call_tool",
        move |conn_id: &str, tool_name: &str, args: rhai::Map| {
            do_call_tool(&ctx, conn_id, tool_name, args)
        },
    );
    let ctx = n_ctx.clone();
    engine.register_fn("mcp_read_resource", move |conn_id: &str, uri: &str| {
        do_read_resource(&ctx, conn_id, uri)
    });
    let ctx = n_ctx.clone();
    engine.register_fn("mcp_list_tools", move |conn_id: &str| {
        do_list_tools(&ctx, conn_id)
    });
    let ctx = n_ctx.clone();
    engine.register_fn("mcp_disconnect", move |conn_id: &str| {
        do_disconnect(&ctx, conn_id)
    });

    // Namespaced `mcp::` aliases for doc parity with the integration spec
    // (mcp::call_tool, mcp::list_tools, ...). They delegate to the same helpers.
    engine.register_static_module("mcp", build_mcp_module(n_ctx).into());
}

/// Builds the `mcp` static module exposing namespaced aliases of the bridge.
fn build_mcp_module(n_ctx: &NativeContext) -> Module {
    let mut module = Module::new();

    let ctx = n_ctx.clone();
    module.set_native_fn(
        "connect",
        move |plugin_name: &str, conn_id: &str, command: &str, args: rhai::Array| {
            do_connect(&ctx, plugin_name, conn_id, command, args)
        },
    );
    let ctx = n_ctx.clone();
    module.set_native_fn(
        "call_tool",
        move |conn_id: &str, tool_name: &str, args: rhai::Map| {
            do_call_tool(&ctx, conn_id, tool_name, args)
        },
    );
    let ctx = n_ctx.clone();
    module.set_native_fn("read_resource", move |conn_id: &str, uri: &str| {
        do_read_resource(&ctx, conn_id, uri)
    });
    let ctx = n_ctx.clone();
    module.set_native_fn("list_tools", move |conn_id: &str| {
        do_list_tools(&ctx, conn_id)
    });
    let ctx = n_ctx.clone();
    module.set_native_fn("disconnect", move |conn_id: &str| {
        do_disconnect(&ctx, conn_id)
    });

    module
}
