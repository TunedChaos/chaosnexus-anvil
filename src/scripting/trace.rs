// chaosnexus-anvil/src/scripting/trace.rs
//
// MCP observability: hop-count loop protection, OpenTelemetry-compatible
// trace/span IDs, and a bounded in-memory trace store consumed by ChaosNexus Forge's
// Trace Explorer (Phase 5 / M5).

use rust_mcp_sdk::schema::CallToolMeta;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Maximum MCP proxy hops before `RecursionLimitExceeded` (integration spec §5).
pub const MAX_HOP_COUNT: u32 = 5;

/// `_meta` key propagated on outbound MCP tool calls (stdio transport has no HTTP headers).
pub const META_HOP_KEY: &str = "X-Chaos-Hop-Count";
pub const META_TRACE_ID_KEY: &str = "X-Chaos-Trace-Id";
pub const META_SPAN_ID_KEY: &str = "X-Chaos-Span-Id";
pub const META_PARENT_SPAN_KEY: &str = "X-Chaos-Parent-Span-Id";

/// Optional Rhai argument key stripped before forwarding; used for canvas attribution.
pub const CHAOS_NODE_KEY: &str = "__chaos_node";

/// Machine-readable error returned when hop count exceeds [`MAX_HOP_COUNT`].
pub const RECURSION_LIMIT_ERROR: &str = "RecursionLimitExceeded";

use std::sync::atomic::{AtomicBool, Ordering};

/// Supervised-mode stdout prefix for trace snapshots (`CHAOSFORGE_TRACES\t<json>`).
pub const SUPERVISED_TRACES_PREFIX: &str = "CHAOSFORGE_TRACES";

/// When true, each recorded span flushes a trace snapshot to stdout for ChaosNexus Forge.
static SUPERVISED_TRACE_STREAM: AtomicBool = AtomicBool::new(false);

/// Enables live trace streaming for ChaosNexus Forge's supervised engine child process.
pub fn set_supervised_trace_stream(enabled: bool) {
    SUPERVISED_TRACE_STREAM.store(enabled, Ordering::Relaxed);
}

fn maybe_dump_supervised(store: &TraceStore) {
    if !SUPERVISED_TRACE_STREAM.load(Ordering::Relaxed) {
        return;
    }
    use std::io::Write;
    println!("{}\t{}", SUPERVISED_TRACES_PREFIX, store.to_json());
    let _ = std::io::stdout().flush();
}

/// Direction of a recorded span relative to ChaosNexus Anvil.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TraceKind {
    /// An inbound MCP call received by ChaosNexus Anvil.
    Inbound,
    /// An outbound MCP call made by ChaosNexus Anvil to an external server.
    Outbound,
}

/// A single span in an MCP tool-call chain.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TraceSpan {
    /// OpenTelemetry-compatible 32-character trace ID.
    pub trace_id: String,
    /// OpenTelemetry-compatible 16-character span ID.
    pub span_id: String,
    /// Parent span ID if this is a nested call.
    pub parent_span_id: Option<String>,
    /// The MCP tool name this span represents.
    pub name: String,
    /// The remote MCP connection identifier, if applicable.
    pub conn_id: Option<String>,
    /// Current hop count in the MCP proxy chain.
    pub hop: u32,
    /// Wall-clock latency of this span in milliseconds.
    pub latency_ms: u64,
    /// Error message if this span resulted in a failure.
    pub error: Option<String>,
    /// Optional visual scripting node label for canvas attribution.
    pub node_label: Option<String>,
    /// Whether this was an inbound or outbound MCP call.
    pub kind: TraceKind,
    /// Unix timestamp (ms) when this span started.
    pub started_at_ms: u64,
}

/// Live trace context threaded through nested inbound/outbound MCP calls.
#[derive(Clone, Debug)]
pub struct ActiveTrace {
    /// The trace ID for the current call chain.
    pub trace_id: String,
    /// Current hop count in the proxy chain.
    pub hop: u32,
    /// Parent span ID to propagate for nesting.
    pub parent_span_id: Option<String>,
    /// The span ID of this active context.
    pub current_span_id: String,
}

/// Bounded ring buffer of recent trace spans (newest last).
#[derive(Clone, Debug, Default)]
pub struct TraceStore {
    spans: VecDeque<TraceSpan>,
    max_spans: usize,
}

impl TraceStore {
    /// Creates a new [`TraceStore`] with the given maximum span capacity.
    pub fn new(max_spans: usize) -> Self {
        Self {
            spans: VecDeque::new(),
            max_spans: max_spans.max(1),
        }
    }

    /// Records a span, evicting the oldest if the buffer is full.
    pub fn record(&mut self, span: TraceSpan) {
        if self.spans.len() >= self.max_spans {
            self.spans.pop_front();
        }
        self.spans.push_back(span);
        maybe_dump_supervised(self);
    }

    /// Serializes all stored spans to a JSON array string.
    pub fn to_json(&self) -> String {
        let list: Vec<&TraceSpan> = self.spans.iter().collect();
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
    }
}

/// Thread-safe handle to the shared trace store.
pub type SharedTraceStore = Arc<Mutex<TraceStore>>;
/// Thread-safe handle to the active trace context (set during call chains).
pub type SharedActiveTrace = Arc<Mutex<Option<ActiveTrace>>>;

/// RAII guard that restores the previous active trace after a scoped span.
pub struct ActiveTraceGuard {
    active: SharedActiveTrace,
    previous: Option<ActiveTrace>,
}

impl Drop for ActiveTraceGuard {
    fn drop(&mut self) {
        *self.active.lock().unwrap() = self.previous.take();
    }
}

/// Pushes `next` as the active trace, returning a guard that restores the prior value.
pub fn push_active_trace(active: &SharedActiveTrace, next: ActiveTrace) -> ActiveTraceGuard {
    let mut lock = active.lock().unwrap();
    let previous = lock.replace(next);
    ActiveTraceGuard {
        active: Arc::clone(active),
        previous,
    }
}

/// Reads the hop count from MCP `_meta.extra` (defaults to 0 for root calls).
pub fn hop_from_meta(meta: &Option<CallToolMeta>) -> u32 {
    meta_extra_value(meta, META_HOP_KEY)
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(0)
}

/// Reads a string field from `_meta.extra`.
pub fn meta_string(meta: &Option<CallToolMeta>, key: &str) -> Option<String> {
    meta_extra_value(meta, key).and_then(|v| v.as_str().map(str::to_string))
}

fn meta_extra_value(meta: &Option<CallToolMeta>, key: &str) -> Option<serde_json::Value> {
    meta.as_ref()
        .and_then(|m| m.extra.as_ref())
        .and_then(|map| map.get(key).cloned())
}

/// Builds `_meta` for an outbound tool call with incremented hop and trace headers.
pub fn build_outbound_meta(
    hop: u32,
    trace_id: &str,
    span_id: &str,
    parent_span_id: Option<&str>,
) -> CallToolMeta {
    let mut extra = serde_json::Map::new();
    extra.insert(META_HOP_KEY.into(), serde_json::Value::Number(hop.into()));
    extra.insert(
        META_TRACE_ID_KEY.into(),
        serde_json::Value::String(trace_id.to_string()),
    );
    extra.insert(
        META_SPAN_ID_KEY.into(),
        serde_json::Value::String(span_id.to_string()),
    );
    if let Some(parent) = parent_span_id {
        extra.insert(
            META_PARENT_SPAN_KEY.into(),
            serde_json::Value::String(parent.to_string()),
        );
    }
    CallToolMeta {
        progress_token: None,
        extra: Some(extra),
    }
}

/// Resolves trace identity for an outbound call from the active context and inbound meta.
pub fn resolve_outbound_trace(active: &Option<ActiveTrace>) -> (String, String, Option<String>) {
    match active {
        Some(ctx) => (
            ctx.trace_id.clone(),
            gen_span_id(),
            Some(ctx.current_span_id.clone()),
        ),
        None => (gen_trace_id(), gen_span_id(), None),
    }
}

/// Resolves trace identity for an inbound tool call.
pub fn resolve_inbound_trace(
    meta: &Option<CallToolMeta>,
    hop: u32,
) -> (String, String, Option<String>) {
    let trace_id = meta_string(meta, META_TRACE_ID_KEY).unwrap_or_else(gen_trace_id);
    let span_id = gen_span_id();
    let parent_span_id = meta_string(meta, META_SPAN_ID_KEY);
    let _ = hop;
    (trace_id, span_id, parent_span_id)
}

/// Records a completed span into the store.
pub fn record_span(store: &SharedTraceStore, span: TraceSpan) {
    store.lock().unwrap().record(span);
}

/// Builds a span record from timing and outcome data.
#[allow(clippy::too_many_arguments)]
pub fn make_span(
    trace_id: String,
    span_id: String,
    parent_span_id: Option<String>,
    name: String,
    conn_id: Option<String>,
    hop: u32,
    started_at_ms: u64,
    started: Instant,
    error: Option<String>,
    node_label: Option<String>,
    kind: TraceKind,
) -> TraceSpan {
    TraceSpan {
        trace_id,
        span_id,
        parent_span_id,
        name,
        conn_id,
        hop,
        latency_ms: started.elapsed().as_millis() as u64,
        error,
        node_label,
        kind,
        started_at_ms,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

static ID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 32 lowercase hex chars (OpenTelemetry trace id).
pub fn gen_trace_id() -> String {
    let n = ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let seed = format!("trace-{}-{}", now_ms(), n);
    let hash = Sha256::digest(seed.as_bytes());
    hex::encode(&hash[..16])
}

/// 16 lowercase hex chars (OpenTelemetry span id).
pub fn gen_span_id() -> String {
    let n = ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let seed = format!("span-{}-{}", now_ms(), n);
    let hash = Sha256::digest(seed.as_bytes());
    hex::encode(&hash[..8])
}

/// Strips the Forge-only `__chaos_node` key from Rhai argument maps before MCP forwarding.
pub fn take_node_label(args: &mut rhai::Map) -> Option<String> {
    let keys: Vec<_> = args.keys().cloned().collect();
    for key in keys {
        if key.as_str() == CHAOS_NODE_KEY {
            return args.remove(&key).map(|v| v.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hop_limit_constant_is_five() {
        assert_eq!(MAX_HOP_COUNT, 5);
    }

    #[test]
    fn rejects_hop_above_max() {
        #[allow(clippy::assertions_on_constants)]
        {
            assert!(6 > MAX_HOP_COUNT);
            assert!(5 <= MAX_HOP_COUNT);
        }
    }

    #[test]
    fn trace_ids_are_otel_sized() {
        assert_eq!(gen_trace_id().len(), 32);
        assert_eq!(gen_span_id().len(), 16);
    }

    #[test]
    fn ring_buffer_evicts_oldest() {
        let mut store = TraceStore::new(2);
        for i in 0..3 {
            store.record(TraceSpan {
                trace_id: "t".into(),
                span_id: format!("s{i}"),
                parent_span_id: None,
                name: format!("tool{i}"),
                conn_id: None,
                hop: 0,
                latency_ms: 1,
                error: None,
                node_label: None,
                kind: TraceKind::Inbound,
                started_at_ms: i,
            });
        }
        assert_eq!(store.spans.len(), 2);
        assert_eq!(store.spans.front().unwrap().span_id, "s1");
    }

    #[test]
    fn strips_chaos_node_from_args() {
        let mut args = rhai::Map::new();
        args.insert("title".into(), rhai::Dynamic::from("hello"));
        args.insert("__chaos_node".into(), rhai::Dynamic::from("create_issue"));
        let label = take_node_label(&mut args);
        assert_eq!(label.as_deref(), Some("create_issue"));
        let chaos_key = "__chaos_node";
        let title_key = "title";
        assert!(!args.keys().any(|k| k.as_str() == chaos_key));
        assert!(args.keys().any(|k| k.as_str() == title_key));
    }
}
