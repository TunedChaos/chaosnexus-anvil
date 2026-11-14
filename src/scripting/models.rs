use crate::scripting::capabilities::CapabilitySet;
use crate::scripting::kv_store::KvStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A live Configuration Variable (CVar) bound to a plugin.
#[derive(Clone, Debug, Serialize)]
pub struct CVar {
    pub plugin_name: String,
    pub name: String,
    pub value: String,
    pub description: String,
}

/// Lightweight configuration parsed during early plugin discovery.
#[derive(Deserialize)]
pub struct PluginConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub main: String,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub cvars: HashMap<String, String>,
}

/// Full plugin metadata extracted from `plugin.toml`.
#[derive(Deserialize, Debug, Clone)]
pub struct PluginMeta {
    pub name: String,
    pub version: Option<String>,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub cvars: HashMap<String, String>,
    #[serde(default)]
    pub capabilities: CapabilitySet,
}

use crate::scripting::plugin_context::PluginCapabilities;
use rhai::{AST, Dynamic};
use rust_mcp_sdk::schema::{Prompt, Resource, Tool};
#[cfg(feature = "database")]
use sea_orm::DatabaseConnection;
#[cfg(not(feature = "database"))]
type DatabaseConnection = ();
use std::sync::{Arc, Mutex, RwLock};

/// Process-wide native context used by Rhai natives and sync bridges.
///
/// Uses `RwLock` (not `OnceLock`) so `PluginManager::new` / `rebuild_from_disk`
/// can replace the live handles. MCP registration writes into this slot; a
/// stale OnceLock left the second manager with an empty tools map while natives
/// still mutated the first context (tests and hot-reload).
static GLOBAL_CONTEXT_SLOT: RwLock<Option<NativeContext>> = RwLock::new(None);

/// Returns a clone of the current global `NativeContext`, if initialized.
pub fn global_context() -> Option<NativeContext> {
    GLOBAL_CONTEXT_SLOT
        .read()
        .ok()
        .and_then(|guard| guard.clone())
}

/// Installs (or replaces) the process-wide native context for Rhai natives.
pub fn set_global_context(ctx: NativeContext) {
    if let Ok(mut slot) = GLOBAL_CONTEXT_SLOT.write() {
        *slot = Some(ctx);
    }
}

// Shared, thread-safe state aliases. These are defined once here (the canonical
// home for scripting models) and re-used by both `NativeContext` and
// `PluginManager`, which intentionally hold the same handles. Naming the deeper
// nestings also satisfies clippy's `type_complexity` lint in a single place.

/// Event name -> list of `(plugin_name, callback_fn)` hooks fired together.
pub type SharedEvents = Arc<Mutex<HashMap<String, Vec<(String, String)>>>>;
/// Webhook port -> request path -> `(plugin_name, callback_fn)` route table.
pub type SharedWebhookRoutes = Arc<RwLock<HashMap<i64, HashMap<String, (String, String)>>>>;
/// `plugin_name -> locale -> phrase_key -> translated string` lookup tree.
pub type SharedTranslations =
    Arc<RwLock<HashMap<String, HashMap<String, HashMap<String, String>>>>>;
/// Live outbound MCP client connections keyed by a script-provided id.
pub type SharedMcpClients =
    Arc<Mutex<HashMap<String, Arc<rust_mcp_sdk::mcp_client::ClientRuntime>>>>;
/// Bounded ring buffer of recent MCP execution trace spans (Phase 5 observability).
pub type SharedTraceStore = Arc<Mutex<crate::scripting::trace::TraceStore>>;
/// Active trace context for nested inbound/outbound MCP calls within a chain.
pub type SharedActiveTrace = Arc<Mutex<Option<crate::scripting::trace::ActiveTrace>>>;

/// The centralized state holding all runtime resources across the ChaosNexus Anvil server.
#[derive(Clone)]
pub struct NativeContext {
    pub asts: Arc<Mutex<HashMap<String, AST>>>,
    pub tools: Arc<Mutex<HashMap<String, Tool>>>,
    pub tool_owners: Arc<Mutex<HashMap<String, String>>>,
    pub db_connections: Arc<Mutex<HashMap<String, DatabaseConnection>>>,
    pub events: SharedEvents,
    pub global_state: Arc<RwLock<HashMap<String, Dynamic>>>,
    pub cvars: Arc<RwLock<HashMap<String, CVar>>>,
    pub natives: Arc<RwLock<HashMap<String, (String, String)>>>,
    pub ws_handles: Arc<Mutex<HashMap<String, tokio::sync::mpsc::Sender<()>>>>,
    pub webhook_handles: Arc<Mutex<HashMap<i64, tokio::sync::mpsc::Sender<()>>>>,
    pub webhook_routes: SharedWebhookRoutes,
    pub timer_tx: tokio::sync::mpsc::UnboundedSender<(String, String, i64, bool, Dynamic)>,
    pub kv_store: Arc<Mutex<Option<KvStore>>>,
    pub resources: Arc<Mutex<HashMap<String, Resource>>>,
    pub resource_owners: Arc<Mutex<HashMap<String, String>>>,
    pub prompts: Arc<Mutex<HashMap<String, Prompt>>>,
    pub prompt_owners: Arc<Mutex<HashMap<String, String>>>,
    pub reload_requested: Arc<Mutex<bool>>,
    pub mcp_log_tx: Option<tokio::sync::mpsc::UnboundedSender<(String, String, String)>>,
    pub translations: SharedTranslations,
    /// Live outbound MCP client connections keyed by a script-provided id.
    /// These let Rhai scripts consume external MCP servers (the "out" half of
    /// the bidirectional bridge). Cleaned up on engine teardown.
    pub mcp_clients: SharedMcpClients,
    /// Recent MCP execution spans for the Trace Explorer (ring buffer).
    pub trace_store: SharedTraceStore,
    /// Nested trace context propagated across inbound plugin tools and outbound
    /// `mcp_call_tool` hops within a single chain.
    pub active_trace: SharedActiveTrace,
    /// Per-plugin granted capabilities (from `plugin.toml [capabilities]`).
    pub plugin_capabilities: PluginCapabilities,
    /// Per-plugin declared prefixes for tool namespacing.
    pub plugin_prefixes: Arc<RwLock<HashMap<String, String>>>,
    /// Global plugin permissions (shell commands and data security).
    pub plugins: Arc<RwLock<HashMap<String, crate::config::PluginConfig>>>,
    /// Connection info (port, token) for the IDE to connect to this instance's SSE stream.
    pub ide_connection_info: Option<(u16, String)>,
}

impl NativeContext {
    /// Retrieves a cloned AST for a plugin, safely releasing the lock immediately.
    pub fn get_ast(&self, plugin_name: &str) -> Option<rhai::AST> {
        self.asts.lock().unwrap().get(plugin_name).cloned()
    }

    /// Retrieves registered callbacks for an event, safely releasing the lock immediately.
    pub fn get_events_for(&self, event_name: &str) -> Vec<(String, String)> {
        self.events.lock().unwrap().get(event_name).cloned().unwrap_or_default()
    }
}
