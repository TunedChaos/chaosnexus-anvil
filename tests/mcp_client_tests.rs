// chaosnexus-anvil/tests/mcp_client_tests.rs
//
// End-to-end verification of the outbound MCP client bridge (Phase 3).
//
// Strategy: launch the freshly-built `chaosnexus-anvil` binary itself as a
// downstream MCP server (over stdio) and drive it through the new `mcp_*`
// native Rhai functions. To keep the run deterministic and side-effect-free we
// relocate the process working directory to an isolated temp dir so the
// downstream instance discovers an EMPTY plugin set, exposing only the built-in
// tools (`chaoswrench_reload_plugins`, `chaoswrench_create_plugin`,
// `chaoswrench_cvars`). The test connects, lists tools, calls a built-in tool, disconnects, and cleans up.

use std::fs;
use std::path::PathBuf;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbound_mcp_client_round_trip() {
    // Isolated working directory: the downstream resolves plugins from
    // `../chaosnexus-scripts/plugins` relative to its (inherited) cwd, so an empty
    // `<tmp>/chaosnexus-scripts/plugins` guarantees a built-in-only tool surface.
    let tmp: PathBuf = std::env::temp_dir().join(format!("cw_mcp_it_{}", std::process::id()));
    let cwd = tmp.join("cwd");
    let scripts = tmp.join("chaosnexus-scripts").join("plugins");
    fs::create_dir_all(&cwd).expect("create cwd");
    fs::create_dir_all(&scripts).expect("create empty plugins dir");
    std::env::set_current_dir(&cwd).expect("set isolated cwd");

    let bin = env!("CARGO_BIN_EXE_chaosnexus-anvil");

    let ctx = chaosnexus_anvil::scripting::engine::empty_context();
    {
        let mut caps = ctx.plugin_capabilities.write().unwrap();
        caps.insert(
            "integration_test".to_string(),
            chaosnexus_anvil::scripting::capabilities::CapabilitySet::from_id_list(&[
                "process_spawn".to_string()
            ]),
        );
    }
    let engine = chaosnexus_anvil::scripting::engine::setup_engine(ctx);

    // Connect -> discover -> invoke -> disconnect, returning "<listed>|<output>".
    let script = format!(
        r#"
        mcp_connect("integration_test", "self", "{bin}", []);
        let tools = mcp_list_tools("self");
        let names = [];
        for t in tools {{ names.push(t.name); }}
        let listed = if names.contains("cn_a_cvars") {{ "true" }} else {{ "false" }};
        let result = mcp_call_tool("self", "cn_a_cvars", #{{}});
        mcp_disconnect("self");
        listed + "|" + result
        "#,
        bin = bin
    );

    let outcome =
        chaosnexus_anvil::scripting::plugin_context::with_plugin_context("integration_test", || {
            engine.eval::<String>(&script)
        });

    // Best-effort cleanup of the isolated artifacts regardless of result.
    let _ = fs::remove_dir_all(&tmp);

    let outcome = outcome.expect("outbound MCP round trip should evaluate without error");
    assert!(
        outcome.starts_with("true|"),
        "expected built-in tool to be listed by downstream server, got: {outcome}"
    );
    assert!(
        outcome.contains("CVar"),
        "expected cvars tool output from downstream server, got: {outcome}"
    );
}

/// The namespaced `mcp::` aliases (Phase 4) delegate to the same bridge helpers
/// as the flat `mcp_*` natives. Calling one on a missing connection must fail
/// with the bridge's own error (not Rhai's "function not found"), proving the
/// `mcp` static module resolved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn namespaced_mcp_aliases_are_registered() {
    let ctx = chaosnexus_anvil::scripting::engine::empty_context();
    let engine = chaosnexus_anvil::scripting::engine::setup_engine(ctx);

    let err = engine
        .eval::<String>(r#"mcp::list_tools("definitely_missing")"#)
        .expect_err("expected a runtime error for a missing connection");
    let msg = err.to_string();
    assert!(
        msg.contains("No MCP client connection"),
        "mcp:: alias did not resolve to the bridge helper, got: {msg}"
    );
}
