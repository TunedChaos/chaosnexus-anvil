// chaosnexus-anvil/src/scripting/manager.rs
use crate::scripting::config_inject::build_plugin_scope;
use crate::scripting::models::{
    CVar, PluginMeta, SharedActiveTrace, SharedEvents, SharedMcpClients, SharedTraceStore,
    SharedTranslations, SharedWebhookRoutes,
};
use crate::scripting::paths;
use rhai::{AST, Dynamic, Engine, Map};
use rust_mcp_sdk::schema::schema_utils::CallToolError;
use rust_mcp_sdk::schema::{
    CallToolResult, LoggingLevel, LoggingMessageNotification, LoggingMessageNotificationParams,
    ServerNotification, TextContent, Tool,
};
#[cfg(feature = "database")]
use sea_orm::DatabaseConnection;
#[cfg(not(feature = "database"))]
type DatabaseConnection = ();

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::broadcast::Sender;

/// Global sender for routing logs directly to connected IDE clients via SSE.
pub static SSE_LOG_SENDER: OnceLock<Sender<ServerNotification>> = OnceLock::new();

/// Initializes the global SSE log sender.
pub fn set_sse_log_sender(sender: Sender<ServerNotification>) {
    let _ = SSE_LOG_SENDER.set(sender);
}

/// The core orchestrator managing the plugin lifecycle, Rhai engine, and global state.
#[allow(dead_code)]
#[derive(Clone)]
pub struct PluginManager {
    scripts_root: std::path::PathBuf,
    engine: Arc<Engine>,
    asts: Arc<std::sync::Mutex<HashMap<String, AST>>>,
    tools: Arc<Mutex<HashMap<String, Tool>>>,
    tool_owners: Arc<Mutex<HashMap<String, String>>>, // tool_name -> plugin_name
    db_connections: Arc<Mutex<HashMap<String, DatabaseConnection>>>,
    events: SharedEvents, // event_name -> Vec<(plugin_name, callback)>
    global_state: Arc<std::sync::RwLock<HashMap<String, rhai::Dynamic>>>,
    cvars: Arc<std::sync::RwLock<HashMap<String, CVar>>>,
    natives: Arc<std::sync::RwLock<HashMap<String, (String, String)>>>, // native_name -> (plugin_name, func_name)
    ws_handles: Arc<std::sync::Mutex<HashMap<String, tokio::sync::mpsc::Sender<()>>>>, // url -> kill_tx
    webhook_handles: Arc<std::sync::Mutex<HashMap<i64, tokio::sync::mpsc::Sender<()>>>>, // port -> kill_tx
    webhook_routes: SharedWebhookRoutes, // port -> path -> (plugin, callback)
    resources: Arc<Mutex<HashMap<String, rust_mcp_sdk::schema::Resource>>>,
    resource_owners: Arc<Mutex<HashMap<String, String>>>,
    prompts: Arc<Mutex<HashMap<String, rust_mcp_sdk::schema::Prompt>>>,
    pub prompt_owners: Arc<Mutex<HashMap<String, String>>>,
    pub reload_requested: Arc<Mutex<bool>>,
    pub mcp_log_tx: Option<tokio::sync::mpsc::UnboundedSender<(String, String, String)>>,
    pub translations: SharedTranslations,
    mcp_clients: SharedMcpClients,
    trace_store: SharedTraceStore,
    active_trace: SharedActiveTrace,
    /// Entry `.rhai` path per loaded plugin (powers assembly-grid canvas lookup).
    plugin_script_paths: Arc<Mutex<HashMap<String, PathBuf>>>,
    pub config: Arc<crate::config::Config>,
}

/// Sentinel prefix for machine-parseable log lines on stderr. A supervising
/// process (ChaosNexus Forge) reads these to stream live logs. The format is
/// `CHAOSFORGE_LOG\t<level>\t<plugin>\t<message>` on a single line.
pub const SUPERVISED_LOG_PREFIX: &str = "CHAOSFORGE_LOG";

/// Emits a single structured log line to stderr for live supervisors.
///
/// stdout is reserved for the MCP stdio transport (and the supervised-mode
/// ready marker), so all streamable diagnostics go to stderr. Newlines in the
/// message are collapsed so each log occupies exactly one line.
pub fn emit_supervised_log(plugin_name: &str, level: &str, msg: &str) {
    let safe_msg = msg.replace(['\n', '\r'], " ");
    eprintln!(
        "{}\t{}\t{}\t{}",
        SUPERVISED_LOG_PREFIX, level, plugin_name, safe_msg
    );
}

/// Routes a log message to stdout (if supervised), SSE (if connected), and local log files.
pub fn write_log(plugin_name: &str, level: &str, msg: &str) {
    use std::io::Write;

    // Mirror to stderr so a live supervisor can stream plugin logs in real time.
    emit_supervised_log(plugin_name, level, msg);

    if let Some(sender) = SSE_LOG_SENDER.get() {
        let mcp_level = match level.to_uppercase().as_str() {
            "ERROR" => LoggingLevel::Error,
            "WARN" | "WARNING" => LoggingLevel::Warning,
            "INFO" => LoggingLevel::Info,
            "DEBUG" => LoggingLevel::Debug,
            _ => LoggingLevel::Info,
        };
        let notif = ServerNotification::LoggingMessageNotification(
            LoggingMessageNotification::new(LoggingMessageNotificationParams {
                level: mcp_level,
                logger: Some(plugin_name.to_string()),
                data: serde_json::json!({
                    "message": msg
                }),
                meta: None,
            }),
        );
        let _ = sender.send(notif);
    }

    let _ = std::fs::create_dir_all("logs");
    let date = chrono::Local::now().format("%Y%m%d").to_string();
    let filepath = format!("logs/L{}.log", date);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(filepath)
    {
        let timestamp = chrono::Local::now()
            .format("%m/%d/%Y - %H:%M:%S")
            .to_string();
        let _ = writeln!(
            file,
            "L {} - [{}]: [{}] {}",
            timestamp, level, plugin_name, msg
        );
    }
}

impl PluginManager {
    /// Root directory passed to `PluginManager::new` (parent of `plugins/` and `lib/`).
    pub fn scripts_root(&self) -> &Path {
        &self.scripts_root
    }

    /// Constructs a new `PluginManager` by discovering and compiling all plugins in `dir_path`.
    pub fn new(
        dir_path: &str,
        mcp_log_tx: Option<tokio::sync::mpsc::UnboundedSender<(String, String, String)>>,
        ide_connection_info: Option<(u16, String)>,
        config: Arc<crate::config::Config>,
    ) -> Self {
        let scripts_root = Path::new(dir_path).to_path_buf();
        paths::init(&scripts_root);
        let tools_ref = Arc::new(Mutex::new(HashMap::new()));
        let tool_owners_ref = Arc::new(Mutex::new(HashMap::new()));
        let db_connections = Arc::new(Mutex::new(HashMap::new()));
        let events_ref = Arc::new(Mutex::new(HashMap::new()));
        let asts_ref = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let global_state_ref = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let cvars_ref = Arc::new(std::sync::RwLock::new(HashMap::new()));

        // Pre-seed launch-time CVar overrides from `<scripts_root>/cvars.toml`
        // BEFORE plugins load, so they take precedence over plugin.toml and
        // code defaults and are visible to `on_plugin_start`. See
        // `scripting::cvars` for the full precedence model.
        {
            let overrides = crate::scripting::cvars::load_overrides(&scripts_root);
            if !overrides.is_empty() {
                eprintln!(
                    "[chaosnexus-anvil] Applying {} launch CVar override(s) from {}",
                    overrides.len(),
                    crate::scripting::cvars::CVAR_CONFIG_FILENAME
                );
                let mut c = cvars_ref.write().unwrap();
                for (name, value) in overrides {
                    c.insert(
                        name.clone(),
                        CVar {
                            plugin_name: String::new(),
                            name,
                            value,
                            description: String::new(),
                        },
                    );
                }
            }
        }
        let natives_ref = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let ws_handles_ref = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let webhook_handles_ref = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let webhook_routes_ref = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let (timer_tx, mut timer_rx) =
            tokio::sync::mpsc::unbounded_channel::<(String, String, i64, bool, rhai::Dynamic)>();

        let mut kv_store_opt = None;
        #[cfg(feature = "database")]
        if let Ok(url) = std::env::var("CHAOSWRENCH_VALKEY_URL") {
            let u = if url.is_empty() {
                "redis://127.0.0.1:6379"
            } else {
                &url
            };
            if let Ok(client) = redis::Client::open(u) {
                eprintln!("[chaosnexus-anvil] Connected to Valkey/Redis at {}", u);
                kv_store_opt = Some(crate::scripting::kv_store::KvStore::Redis(client));
            }
        }
        
        if kv_store_opt.is_none() {
            let path = std::path::Path::new("./.chaoswrench_data/sled_db");
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(db) = sled::open(path) {
                eprintln!("[chaosnexus-anvil] Initialized Sled at {:?}", path);
                kv_store_opt = Some(crate::scripting::kv_store::KvStore::Sled(db));
            }
        }
        let kv_store = Arc::new(Mutex::new(kv_store_opt));
        let resources_ref = Arc::new(Mutex::new(HashMap::new()));
        let resource_owners_ref = Arc::new(Mutex::new(HashMap::new()));
        let prompts_ref = Arc::new(Mutex::new(HashMap::new()));
        let prompt_owners_ref = Arc::new(Mutex::new(HashMap::new()));
        let reload_requested = Arc::new(Mutex::new(false));
        let translations_ref = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let mcp_clients_ref = Arc::new(Mutex::new(HashMap::new()));
        let trace_store_ref = Arc::new(Mutex::new(crate::scripting::trace::TraceStore::new(256)));
        let active_trace_ref = Arc::new(Mutex::new(None));
        let plugin_capabilities_ref = crate::scripting::plugin_context::empty_plugin_capabilities();
        let plugin_prefixes_ref = Arc::new(std::sync::RwLock::new(HashMap::new()));

        let mut plugins_map = HashMap::new();
        if let Some(plugins) = &config.plugins {
            for (plugin_name, plugin_config) in plugins {
                plugins_map.insert(plugin_name.clone(), plugin_config.clone());
            }
        }
        let plugins_ref = Arc::new(std::sync::RwLock::new(plugins_map));

        let context = crate::scripting::models::NativeContext {
            asts: Arc::clone(&asts_ref),
            tools: Arc::clone(&tools_ref),
            tool_owners: Arc::clone(&tool_owners_ref),
            db_connections: Arc::clone(&db_connections),
            events: Arc::clone(&events_ref),
            global_state: Arc::clone(&global_state_ref),
            cvars: Arc::clone(&cvars_ref),
            natives: Arc::clone(&natives_ref),
            ws_handles: Arc::clone(&ws_handles_ref),
            webhook_handles: Arc::clone(&webhook_handles_ref),
            webhook_routes: Arc::clone(&webhook_routes_ref),
            timer_tx,
            kv_store: kv_store.clone(),
            resources: resources_ref.clone(),
            resource_owners: resource_owners_ref.clone(),
            prompts: prompts_ref.clone(),
            prompt_owners: prompt_owners_ref.clone(),
            reload_requested: reload_requested.clone(),
            mcp_log_tx: mcp_log_tx.clone(),
            translations: translations_ref.clone(),
            mcp_clients: mcp_clients_ref.clone(),
            trace_store: trace_store_ref.clone(),
            active_trace: active_trace_ref.clone(),
            plugin_capabilities: plugin_capabilities_ref.clone(),
            plugin_prefixes: plugin_prefixes_ref.clone(),
            plugins: plugins_ref.clone(),
            ide_connection_info,
        };
        // Always replace so rebuild_from_disk / tests share the same tools map
        // as Rhai natives that read the process-wide slot.
        crate::scripting::models::set_global_context(context.clone());
        let engine = crate::scripting::engine::setup_engine(context);
        let engine_arc = Arc::new(engine);

        let mut plugin_metas = HashMap::new();
        let mut plugin_paths = HashMap::new();

        eprintln!("[chaosnexus-anvil] Discovering plugins in: {}", dir_path);
        let path = Path::new(dir_path);
        let plugins_dir = path.join("plugins");

        if let Ok(entries) = std::fs::read_dir(&plugins_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if !p.is_dir() {
                    continue;
                }
                let plugin_name = p.file_name().unwrap().to_string_lossy().to_string();
                if plugin_name == "disabled" {
                    continue;
                }
                let meta_path = p.join("plugin.toml");
                let script_path = p.join(format!("{}_tool.rhai", plugin_name));

                if !meta_path.exists() || !script_path.exists() {
                    eprintln!(
                        "[warn] Skipping plugin dir {:?} (missing plugin.toml or entry script)",
                        p
                    );
                    continue;
                }

                let Ok(meta_str) = std::fs::read_to_string(&meta_path) else {
                    eprintln!("[warn] Failed to read plugin.toml in {:?}", p);
                    continue;
                };

                let Ok(meta) = toml::from_str::<PluginMeta>(&meta_str) else {
                    eprintln!("[warn] Failed to parse plugin.toml in {:?}", p);
                    continue;
                };

                eprintln!(
                    "[chaosnexus-anvil] Found plugin: {} v{:?}",
                    meta.name, meta.version
                );

                let prefix = meta.prefix.clone().unwrap_or_else(|| plugin_name.clone());
                plugin_prefixes_ref.write().unwrap().insert(plugin_name.clone(), prefix);

                // Seed CVars from plugin.toml as defaults. A
                // launch override (pre-seeded from cvars.toml)
                // already present must win, so only insert when
                // the key is absent; otherwise just attribute
                // the override to this plugin.
                {
                    let mut c = cvars_ref.write().unwrap();
                    for (k, v) in &meta.cvars {
                        match c.get_mut(k) {
                            Some(existing) => {
                                if existing.plugin_name.is_empty() {
                                    existing.plugin_name = meta.name.clone();
                                }
                            }
                            None => {
                                c.insert(
                                    k.clone(),
                                    CVar {
                                        plugin_name: meta.name.clone(),
                                        name: k.clone(),
                                        value: v.clone(),
                                        description: String::new(), // filled by register_cvar if called
                                    },
                                );
                            }
                        }
                    }
                }

                plugin_metas.insert(meta.name.clone(), meta.clone());
                plugin_paths.insert(plugin_name.clone(), script_path);

                let mut effective_caps = meta.capabilities.clone();
                let mut is_authorized = false;
                
                if let Some(plugins) = &config.plugins
                    && let Some(pconfig) = plugins.get(&plugin_name) {
                        if let Some(granted) = &pconfig.granted_capabilities {
                            is_authorized = true;
                            effective_caps.granted.retain(|cap| granted.contains(&cap.as_str().to_string()));
                        }
                        if let Some(env_allowed) = &pconfig.env_allowlist {
                            effective_caps.env_allowlist = env_allowed.clone();
                        } else {
                            effective_caps.env_allowlist.clear();
                        }
                    }
                
                if !is_authorized {
                    effective_caps.granted.clear();
                    effective_caps.env_allowlist.clear();
                }

                if let Ok(mut caps_map) = plugin_capabilities_ref.write() {
                    caps_map.insert(plugin_name.clone(), effective_caps);
                }

                // Load Translations
                let mut plugin_translations = HashMap::new();
                let translations_dir = p.join("translations");
                if let Ok(trans_entries) = std::fs::read_dir(&translations_dir) {
                    for t_entry in trans_entries.flatten() {
                        let t_path = t_entry.path();
                        if !t_path.is_file() {
                            continue;
                        }
                        if t_path.extension().map(|s| s == "toml").unwrap_or(false) {
                            let locale = t_path.file_stem().unwrap().to_string_lossy().to_string();

                            let Ok(t_content) = std::fs::read_to_string(&t_path) else {
                                continue;
                            };

                            let Ok(t_map) = toml::from_str::<HashMap<String, String>>(&t_content)
                            else {
                                eprintln!("[warn] Failed to parse translation file {:?}", t_path);
                                continue;
                            };

                            plugin_translations.insert(locale, t_map);
                        }
                    }
                }
                translations_ref
                    .write()
                    .unwrap()
                    .insert(meta.name.clone(), plugin_translations);
            }
        }

        // Topological Sort
        let order = Self::resolve_order(&plugin_metas).unwrap_or_else(|e| {
            eprintln!("[error] {}", e);
            vec![]
        });

        eprintln!("[chaosnexus-anvil] Load Order: {:?}", order);

        // Compile & Initialize
        for p_name in &order {
            if let Some(script_path) = plugin_paths.get(p_name) {
                if let Ok(mut ast) = engine_arc.compile_file(script_path.clone()) {
                    ast.set_source(p_name);
                    asts_ref.lock().unwrap().insert(p_name.clone(), ast.clone());

                    // Run `on_plugin_start()`, prefer the assembly grid when a canvas
                    // topology is bound to the entry function. Scope carries the
                    // identity-scoped `CONFIG` constant for this plugin.
                    let mut scope = build_plugin_scope(p_name);

                    let grid_ran = Self::run_lifecycle_grid(
                        &engine_arc,
                        &plugin_paths,
                        &ast,
                        &mut scope,
                        p_name,
                        "on_plugin_start",
                        Map::new(),
                    );
                    let res = if grid_ran {
                        Ok(())
                    } else {
                        crate::scripting::plugin_context::with_plugin_context(p_name, || {
                            engine_arc.call_fn::<()>(&mut scope, &ast, "on_plugin_start", ())
                        })
                    };
                    if let Err(e) = res {
                        eprintln!("[error] Plugin {} failed to start: {}", p_name, e);
                        // FAILSafe: Unload the plugin so it doesn't receive further events
                        asts_ref.lock().unwrap().remove(p_name);
                        eprintln!("[warn] Plugin {} was aborted and removed.", p_name);
                    }
                } else if let Err(e) = engine_arc.compile_file(script_path.clone()) {
                    eprintln!("[error] Failed to compile plugin {}: {}", p_name, e);
                }
            }
        }

        // Final Ready State
        let cloned_asts: Vec<(String, AST)> = {
            let guard = asts_ref.lock().unwrap();
            order
                .iter()
                .filter_map(|p_name| guard.get(p_name).map(|ast| (p_name.clone(), ast.clone())))
                .collect()
        };
        for (p_name, ast) in cloned_asts {
            let mut scope = build_plugin_scope(&p_name);
            let _ = crate::scripting::plugin_context::with_plugin_context(&p_name, || {
                engine_arc.call_fn::<()>(&mut scope, &ast, "on_all_plugins_loaded", ())
            });
        }

        let engine_clone_for_timers = engine_arc.clone();
        let asts_clone_for_timers = asts_ref.clone();

        tokio::spawn(async move {
            while let Some((p_name, cb_name, ms, repeat, payload)) = timer_rx.recv().await {
                let e = engine_clone_for_timers.clone();
                let a = asts_clone_for_timers.clone();
                tokio::spawn(async move {
                    Self::run_timer_loop(e, a, p_name, cb_name, ms, repeat, payload).await;
                });
            }
        });

        Self {
            scripts_root,
            engine: engine_arc,
            asts: asts_ref,
            tools: tools_ref,
            tool_owners: tool_owners_ref,
            db_connections,
            events: events_ref,
            global_state: global_state_ref,
            cvars: cvars_ref,
            natives: natives_ref,
            ws_handles: ws_handles_ref,
            webhook_handles: webhook_handles_ref,
            webhook_routes: webhook_routes_ref,
            resources: resources_ref,
            resource_owners: resource_owners_ref,
            prompts: prompts_ref,
            prompt_owners: prompt_owners_ref,
            reload_requested,
            mcp_log_tx,
            translations: translations_ref,
            mcp_clients: mcp_clients_ref,
            trace_store: trace_store_ref,
            active_trace: active_trace_ref,
            plugin_script_paths: Arc::new(Mutex::new(plugin_paths)),
            config,
        }
    }

    /// Attempts to run `entry_fn` through the assembly-line canvas when a
    /// sidecar topology exists. Returns `true` when the grid handled execution.
    fn run_lifecycle_grid(
        engine: &Engine,
        plugin_paths: &HashMap<String, PathBuf>,
        ast: &AST,
        scope: &mut rhai::Scope,
        plugin_name: &str,
        entry_fn: &str,
        entry_args: Map,
    ) -> bool {
        use crate::scripting::graph::{
            executor::execute_assembly_grid, load_canvas_sidecar,
            manifest::extract_function_signatures, plan::find_entry_node,
        };

        let Some(script_path) = plugin_paths.get(plugin_name) else {
            return false;
        };
        let Some(canvas) = load_canvas_sidecar(script_path) else {
            return false;
        };
        if !canvas.has_executable_topology() || find_entry_node(&canvas, entry_fn).is_none() {
            return false;
        }
        let Ok(source) = std::fs::read_to_string(script_path) else {
            return false;
        };
        let Ok(sigs) = extract_function_signatures(&source) else {
            return false;
        };
        let sig_map: HashMap<String, Vec<String>> =
            sigs.into_iter().map(|s| (s.name, s.params)).collect();

        match crate::scripting::plugin_context::with_plugin_context(plugin_name, || {
            execute_assembly_grid(engine, scope, ast, &canvas, &sig_map, entry_fn, entry_args)
        }) {
            Ok(_) => true,
            Err(e) => {
                eprintln!(
                    "[warn] Assembly grid for {}::{} failed: {}; falling back to direct call",
                    plugin_name, entry_fn, e
                );
                false
            }
        }
    }

    /// Executes a plugin's callback function repeatedly or once after a delay.
    async fn run_timer_loop(
        engine: Arc<Engine>,
        asts: Arc<Mutex<HashMap<String, AST>>>,
        plugin_name: String,
        callback_name: String,
        delay_ms: i64,
        repeat: bool,
        payload: Dynamic,
    ) {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms as u64)).await;

            let target_ast = {
                let asts_guard = asts.lock().unwrap();
                asts_guard.get(&plugin_name).cloned()
            };

            let Some(ast) = target_ast else {
                break; // Plugin was unloaded or doesn't exist
            };

            let e_clone = engine.clone();
            let p_name_clone = plugin_name.clone();
            let cb_name_clone = callback_name.clone();
            let payload_clone = payload.clone();
            
            let _ = tokio::task::spawn_blocking(move || {
                crate::scripting::plugin_context::with_plugin_context(
                    &p_name_clone,
                    || {
                        let mut scope = build_plugin_scope(&p_name_clone);
                        let _ = e_clone.call_fn::<()>(
                            &mut scope,
                            &ast,
                            &cb_name_clone,
                            (payload_clone,),
                        );
                    },
                );
            })
            .await;

            if !repeat {
                break;
            }
        }
    }

    /// Runs `entry_fn` via the assembly grid when configured; `None` means fall back.
    fn try_execute_grid(
        &self,
        plugin_name: &str,
        entry_fn: &str,
        entry_args: Map,
    ) -> Option<Result<Dynamic, String>> {
        use crate::scripting::graph::{
            executor::execute_assembly_grid, load_canvas_sidecar,
            manifest::extract_function_signatures, plan::find_entry_node,
        };

        let script_path = self
            .plugin_script_paths
            .lock()
            .ok()?
            .get(plugin_name)?
            .clone();
        let canvas = load_canvas_sidecar(&script_path)?;
        if !canvas.has_executable_topology() || find_entry_node(&canvas, entry_fn).is_none() {
            return None;
        }
        let ast = self.asts.lock().ok()?.get(plugin_name)?.clone();
        let source = std::fs::read_to_string(&script_path).ok()?;
        let sigs = extract_function_signatures(&source).ok()?;
        let sig_map: HashMap<String, Vec<String>> =
            sigs.into_iter().map(|s| (s.name, s.params)).collect();
        let mut scope = build_plugin_scope(plugin_name);
        Some(crate::scripting::plugin_context::with_plugin_context(
            plugin_name,
            || {
                execute_assembly_grid(
                    &self.engine,
                    &mut scope,
                    &ast,
                    &canvas,
                    &sig_map,
                    entry_fn,
                    entry_args,
                )
            },
        ))
    }

    /// Fires `on_plugin_stop` for all loaded plugins and gracefully shuts down connections.
    pub fn stop_all(&self) {
        let cloned_asts: Vec<(String, AST)> = {
            let guard = self.asts.lock().unwrap();
            guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        for (p_name, ast) in cloned_asts.iter() {
            let mut scope = build_plugin_scope(p_name);
            let _ = crate::scripting::plugin_context::with_plugin_context(p_name, || {
                self.engine
                    .call_fn::<()>(&mut scope, ast, "on_plugin_stop", ())
            });
        }

        // Close all WebSockets gracefully
        let mut handles = self.ws_handles.lock().unwrap();
        for (_, tx) in handles.drain() {
            let _ = tx.try_send(());
        }

        // Close all Webhook Servers
        let mut webhook_handles = self.webhook_handles.lock().unwrap();
        for (_, tx) in webhook_handles.drain() {
            let _ = tx.try_send(());
        }

        // Gracefully shut down all downstream MCP client connections so reloads
        // never leak child processes. We block on each shutdown via the same
        // thread-bridge used elsewhere, since stop_all is synchronous.
        let clients: Vec<_> = {
            let mut map = self.mcp_clients.lock().unwrap();
            map.drain().map(|(_, c)| c).collect()
        };
        for client in clients {
            use rust_mcp_sdk::McpClient;
            let _ = crate::scripting::utils::run_async(async move { client.shut_down().await });
        }
    }

    /// Topologically sorts plugins based on their declared dependencies.
    fn resolve_order(plugins: &HashMap<String, PluginMeta>) -> Result<Vec<String>, String> {
        let mut order = Vec::new();
        let mut visited = HashSet::new();
        let mut visiting = HashSet::new();

        fn visit(
            name: &str,
            plugins: &HashMap<String, PluginMeta>,
            visited: &mut HashSet<String>,
            visiting: &mut HashSet<String>,
            order: &mut Vec<String>,
        ) -> Result<(), String> {
            if visiting.contains(name) {
                return Err(format!(
                    "Circular dependency detected involving plugin: {}",
                    name
                ));
            }
            if visited.contains(name) {
                return Ok(());
            }

            visiting.insert(name.to_string());

            if let Some(meta) = plugins.get(name) {
                for dep in &meta.dependencies {
                    if !plugins.contains_key(dep) {
                        return Err(format!("Plugin {} depends on missing plugin {}", name, dep));
                    }
                    visit(dep, plugins, visited, visiting, order)?;
                }
            }

            visiting.remove(name);
            visited.insert(name.to_string());
            order.push(name.to_string());
            Ok(())
        }

        for name in plugins.keys() {
            visit(name, plugins, &mut visited, &mut visiting, &mut order)?;
        }

        Ok(order)
    }

    /// Dumps all dynamically registered tools into the provided list for the MCP server.
    pub fn register_tools(&self, tools: &mut Vec<Tool>) {
        let t_map = self.tools.lock().unwrap();
        for tool in t_map.values() {
            tools.push(tool.clone());
        }
    }

    /// Returns true when a plugin has already registered the given MCP tool name.
    pub fn tool_exists(&self, name: &str) -> bool {
        self.tools.lock().unwrap().contains_key(name)
    }

    /// Returns true when a discovered plugin finished loading (AST retained).
    pub fn plugin_loaded(&self, name: &str) -> bool {
        self.asts.lock().unwrap().contains_key(name)
    }

    /// Stops all plugins and rebuilds the manager from the on-disk scripts root.
    pub fn rebuild_from_disk(&mut self) {
        let scripts_root = self.scripts_root().to_string_lossy().into_owned();
        let mcp_log_tx = self.mcp_log_tx.clone();
        let ide_connection_info = crate::scripting::models::global_context()
            .and_then(|ctx| ctx.ide_connection_info.clone());
        let config = Arc::clone(&self.config);
        self.stop_all();
        *self = Self::new(&scripts_root, mcp_log_tx, ide_connection_info, config);
    }

    /// Shared dispatch for owner-routed plugin entrypoints (tools, resources,
    /// and prompts all follow the same skeleton). Resolves the plugin that owns
    /// `key` within `owners`, clones its compiled AST, and invokes the named
    /// Rhai function on a blocking worker so it never stalls the async runtime.
    ///
    /// Returns `Ok(None)` when either no owner is registered for `key` or that
    /// owner has no live AST (e.g. it failed to start), preserving the
    /// "unknown -> None" contract the individual handlers relied on. Script
    /// failures and join errors are surfaced as `Err(String)`.
    async fn dispatch_to_owner<A>(
        &self,
        owners: &Arc<Mutex<HashMap<String, String>>>,
        key: &str,
        fn_name: &'static str,
        args: A,
    ) -> Result<Option<String>, String>
    where
        A: rhai::FuncArgs + Send + 'static,
    {
        // Return-early if nothing owns this key or the owner has no live AST.
        let Some(plugin_name) = owners.lock().unwrap().get(key).cloned() else {
            return Ok(None);
        };
        let Some(ast) = self.asts.lock().unwrap().get(&plugin_name).cloned() else {
            return Ok(None);
        };

        let engine = self.engine.clone();
        let plugin_for_ctx = plugin_name.clone();
        let res = tokio::task::spawn_blocking(move || {
            crate::scripting::plugin_context::with_plugin_context(&plugin_for_ctx, || {
                let mut scope = build_plugin_scope(&plugin_for_ctx);
                engine.call_fn::<String>(&mut scope, &ast, fn_name, args)
            })
        })
        .await
        .map_err(|e| e.to_string())?;

        res.map(Some).map_err(|e| format!("Script error: {}", e))
    }

    /// Dispatches an incoming MCP tool call request to the owning plugin's Rhai function.
    pub async fn handle_tool(
        &self,
        name: &str,
        params: &rust_mcp_sdk::schema::CallToolRequestParams,
    ) -> Result<Option<CallToolResult>, CallToolError> {
        use crate::scripting::trace::{
            RECURSION_LIMIT_ERROR, TraceKind, hop_from_meta, make_span, push_active_trace,
            record_span, resolve_inbound_trace,
        };
        use std::time::Instant;

        let hop = hop_from_meta(&params.meta);
        if hop > crate::scripting::trace::MAX_HOP_COUNT {
            return Err(CallToolError::from_message(RECURSION_LIMIT_ERROR));
        }

        let (trace_id, span_id, parent_span_id) = resolve_inbound_trace(&params.meta, hop);
        let started_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let timer = Instant::now();

        let active = crate::scripting::trace::ActiveTrace {
            trace_id: trace_id.clone(),
            hop,
            parent_span_id: parent_span_id.clone(),
            current_span_id: span_id.clone(),
        };
        let _trace_guard = push_active_trace(&self.active_trace, active);

        let mut rhai_args = Map::new();
        if let Some(args) = &params.arguments {
            for (k, v) in args {
                // Recursively convert nested JSON objects/arrays into native
                // `rhai::Map`/`rhai::Array` structures. `serde_json::Value`
                // implements `serde::Serialize`, so `to_dynamic` handles deep
                // trees without manual stringification or custom match logic.
                let dynamic =
                    rhai::serde::to_dynamic(v).unwrap_or_else(|_| Dynamic::from(v.to_string()));
                rhai_args.insert(k.clone().into(), dynamic);
            }
        }

        let owner_plugin = self.tool_owners.lock().unwrap().get(name).cloned();
        let dispatch_result = match owner_plugin {
            Some(plugin_name) => {
                match self.try_execute_grid(&plugin_name, "execute", rhai_args.clone()) {
                    Some(Ok(dyn_val)) => dyn_val
                        .into_string()
                        .map(Some)
                        .map_err(|e| format!("Grid result is not a string: {}", e)),
                    Some(Err(e)) => Err(e),
                    None => {
                        self.dispatch_to_owner(
                            &self.tool_owners,
                            name,
                            "execute",
                            (name.to_string(), rhai_args),
                        )
                        .await
                    }
                }
            }
            None => Ok(None),
        };

        let error = dispatch_result.as_ref().err().cloned();
        record_span(
            &self.trace_store,
            make_span(
                trace_id,
                span_id,
                parent_span_id,
                name.to_string(),
                None,
                hop,
                started_at_ms,
                timer,
                error,
                Some(name.to_string()),
                TraceKind::Inbound,
            ),
        );

        match dispatch_result {
            Ok(Some(output)) => Ok(Some(CallToolResult::text_content(vec![TextContent::from(
                output,
            )]))),
            Ok(None) => Ok(None),
            Err(e) => Err(CallToolError::from_message(e)),
        }
    }

    /// Returns a human-readable summary of all live CVars.
    pub fn list_cvars(&self) -> String {
        let c = self.cvars.read().unwrap();
        let mut out = String::new();
        for (k, v) in c.iter() {
            out.push_str(&format!(
                "CVar: {}\nPlugin: {}\nValue: {}\nDescription: {}\n\n",
                k, v.plugin_name, v.value, v.description
            ));
        }
        if out.is_empty() {
            out.push_str("No CVars registered.");
        }
        out
    }

    /// Serializes the live CVar registry as a compact JSON array for ChaosNexus Forge
    /// (the CVar Controller IPC dump). See [`crate::scripting::cvars::to_json`].
    pub fn cvars_json(&self) -> String {
        let c = self.cvars.read().unwrap();
        crate::scripting::cvars::to_json(&c)
    }

    /// Serializes the bounded MCP trace ring buffer for ChaosNexus Forge Trace Explorer.
    pub fn traces_json(&self) -> String {
        self.trace_store.lock().unwrap().to_json()
    }

    /// Persists current CVar values to `<scripts_root>/cvars.toml` so edits made
    /// from ChaosNexus Forge survive an engine restart.
    pub fn persist_cvars(&self) -> Result<String, String> {
        let c = self.cvars.read().unwrap();
        crate::scripting::cvars::persist(&self.scripts_root, &c)
    }

    /// Updates a CVar dynamically and fires the `on_cvar_changed` event.
    pub fn set_cvar(&self, name: &str, value: &str) -> Result<String, String> {
        let mut c = self.cvars.write().unwrap();
        if let Some(cvar) = c.get_mut(name) {
            cvar.value = value.to_string();

            // Fire event
            let mut event_args = rhai::Map::new();
            event_args.insert("name".into(), rhai::Dynamic::from(name.to_string()));
            event_args.insert("value".into(), rhai::Dynamic::from(value.to_string()));
            let dynamic_args = rhai::Dynamic::from(event_args);

            // Need to drop write lock before firing event to prevent deadlocks if the hook reads cvars
            drop(c);

            let _ = self.fire_event_native("on_cvar_changed", dynamic_args);
            return Ok(format!(
                "Successfully updated CVar '{}' to '{}'.",
                name, value
            ));
        }
        Err(format!("CVar '{}' not found.", name))
    }

    /// Dispatches an event payload to all registered plugin listeners synchronously.
    pub fn fire_event_native(
        &self,
        event_name: &str,
        payload: rhai::Dynamic,
    ) -> Result<(), String> {
        let events = self.events.lock().unwrap();
        let mut calls_to_make = Vec::new();
        if let Some(listeners) = events.get(event_name) {
            for (plugin_name, callback) in listeners {
                calls_to_make.push((plugin_name.clone(), callback.clone()));
            }
        }
        drop(events);

        let mut any_err = String::new();
        for (p_name, cb) in calls_to_make {
            let ast = self.asts.lock().unwrap().get(&p_name).cloned();
            if let Some(ast) = ast {
                let mut scope = build_plugin_scope(&p_name);

                // Use the fully-configured engine (with all native APIs) so
                // event hooks such as `on_cvar_changed` can call natives like
                // `log_info`. A bare `Engine::new()` would lack registrations
                // and fail every native call inside the hook. The hook runs
                // under its owning plugin's trusted identity so capability gates
                // and `CONFIG` resolve correctly.
                let result = crate::scripting::plugin_context::with_plugin_context(&p_name, || {
                    self.engine
                        .call_fn::<()>(&mut scope, &ast, &cb, (payload.clone(),))
                });
                if let Err(e) = result {
                    any_err.push_str(&format!("{} -> {}\n", p_name, e));
                }
            }
        }

        if any_err.is_empty() {
            Ok(())
        } else {
            Err(any_err)
        }
    }

    /// Appends all dynamically registered resources into the provided vector for the MCP server.
    pub fn register_resources(&self, resources: &mut Vec<rust_mcp_sdk::schema::Resource>) {
        let r_map = self.resources.lock().unwrap();
        for res in r_map.values() {
            resources.push(res.clone());
        }
    }

    /// Appends all dynamically registered prompts into the provided vector for the MCP server.
    pub fn register_prompts(&self, prompts: &mut Vec<rust_mcp_sdk::schema::Prompt>) {
        let p_map = self.prompts.lock().unwrap();
        for p in p_map.values() {
            prompts.push(p.clone());
        }
    }

    /// Dispatches an incoming resource read request to the owning plugin's Rhai function.
    pub async fn handle_resource(&self, uri: &str) -> Result<Option<String>, String> {
        self.dispatch_to_owner(
            &self.resource_owners,
            uri,
            "execute_resource",
            (uri.to_string(),),
        )
        .await
    }

    /// Dispatches an incoming prompt retrieval request to the owning plugin's Rhai function.
    pub async fn handle_prompt(
        &self,
        name: &str,
        params: &rust_mcp_sdk::schema::GetPromptRequestParams,
    ) -> Result<Option<rust_mcp_sdk::schema::GetPromptResult>, String> {
        let mut rhai_args = rhai::Map::new();
        if let Some(args) = &params.arguments {
            for (k, v) in args {
                rhai_args.insert(k.clone().into(), rhai::Dynamic::from(v.to_string()));
            }
        }

        match self
            .dispatch_to_owner(
                &self.prompt_owners,
                name,
                "execute_prompt",
                (name.to_string(), rhai_args),
            )
            .await?
        {
            Some(output) => {
                let msg = rust_mcp_sdk::schema::PromptMessage {
                    role: rust_mcp_sdk::schema::Role::User,
                    content: rust_mcp_sdk::schema::ContentBlock::text_content(output),
                };
                Ok(Some(rust_mcp_sdk::schema::GetPromptResult {
                    description: None,
                    messages: vec![msg],
                    meta: None,
                }))
            }
            None => Ok(None),
        }
    }
}
