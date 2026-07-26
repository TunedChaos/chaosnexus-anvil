// chaosnexus-anvil/tests/trace_hop_tests.rs
//
// Verifies Phase 5 hop-limit protection on the outbound MCP bridge.

use chaosnexus_anvil::scripting::trace::{MAX_HOP_COUNT, RECURSION_LIMIT_ERROR};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outbound_mcp_rejects_hop_beyond_limit() {
    let ctx = chaosnexus_anvil::scripting::engine::empty_context();

    // Seed an active trace at the maximum hop so the next outbound call must fail.
    {
        let mut active = ctx.active_trace.lock().unwrap();
        *active = Some(chaosnexus_anvil::scripting::trace::ActiveTrace {
            trace_id: "abc".into(),
            hop: MAX_HOP_COUNT,
            parent_span_id: None,
            current_span_id: "span".into(),
        });
    }

    let engine = chaosnexus_anvil::scripting::engine::setup_engine(ctx);

    let err = engine
        .eval::<String>(
            r#"
            mcp_call_tool("missing_conn", "any_tool", #{});
        "#,
        )
        .expect_err("expected hop limit before connection lookup");
    let msg = err.to_string();
    assert!(
        msg.contains(RECURSION_LIMIT_ERROR),
        "expected RecursionLimitExceeded, got: {msg}"
    );
}
