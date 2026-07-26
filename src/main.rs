// chaosnexus-anvil/src/main.rs
use chrono::Utc;
use clap::Parser;
use rust_mcp_sdk::error::SdkResult;
use rust_mcp_sdk::mcp_server::{McpServerOptions, server_runtime};
use rust_mcp_sdk::schema::{
    Implementation, InitializeResult, ProtocolVersion, ServerCapabilities, ServerCapabilitiesTools,
};
use rust_mcp_sdk::{McpServer, ToMcpServerHandler};
use rust_mcp_transport::{StdioTransport, TransportOptions};
use std::io::Write;
use std::sync::Arc;

use chaosnexus_anvil::config::Config;
use chaosnexus_anvil::server::Handler;

/// Logs only when CHAOSWRENCH_DEBUG_LOG is set
fn debug_log(msg: &str) {
    if let Ok(path) = std::env::var("CHAOSWRENCH_DEBUG_LOG") {
        let line = format!("{} [chaosnexus-anvil] {}\n", Utc::now().to_rfc3339(), msg);
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(line.as_bytes()));
    }
}

#[derive(Parser, Debug, Clone)]
#[command(name = "chaosnexus-anvil")]
#[command(about = "ChaosNexus Anvil MCP Server: sandboxed Rhai plugin engine")]
struct Cli {
    /// Optional path to chaosnexus-anvil.toml config file (overrides all other config paths)
    #[arg(long = "config", env = "CHAOSWRENCH_CONFIG")]
    config: Option<String>,

    /// Generates the Rhai metadata schema to JSON and exits
    #[arg(long = "generate-schema")]
    generate_schema: bool,

    /// Prints the generated schema JSON to stdout and exits (used by the docs
    /// generator). Takes precedence over `--generate-schema`.
    #[arg(long = "schema-stdout")]
    schema_stdout: bool,

    /// Extracts the function signatures from a single `.rhai` file and prints
    /// them as a JSON array to stdout, then exits. Powers the ChaosNexus Forge
    /// "Assembly Line" canvas (node handles + stale-binding detection).
    #[arg(long = "list-functions", value_name = "FILE")]
    list_functions: Option<String>,

    /// Runs the engine in supervised mode for ChaosNexus Forge: boots plugins,
    /// streams logs to stderr, and accepts `reload` / `stop` commands on stdin
    /// without starting the MCP stdio transport.
    #[arg(long = "supervised")]
    supervised: bool,

    /// Overrides the scripts root (the directory that contains `plugins/` and
    /// `lib/`). Primarily used by ChaosNexus Forge when launching a supervised engine
    /// for a connected workspace.
    #[arg(long = "scripts-dir")]
    scripts_dir: Option<String>,

    /// Explicitly name this ChaosNexus Anvil instance (used for IDE discovery).
    /// If not provided, a random friendly name is generated.
    #[arg(long = "name")]
    name: Option<String>,
}

/// Publishes the engine's current CVar snapshot to the supervisor on stdout as a
/// single tab-delimited `CHAOSFORGE_CVARS` line whose payload is the JSON array.
/// ChaosNexus Forge parses this into the `engine://cvars` event consumed by the CVar
/// Controller.
fn dump_cvars(manager: &chaosnexus_anvil::scripting::manager::PluginManager) {
    use std::io::Write;
    println!("CHAOSFORGE_CVARS\t{}", manager.cvars_json());
    let _ = std::io::stdout().flush();
}

/// Publishes the engine's MCP trace ring buffer to the supervisor on stdout.
fn dump_traces(manager: &chaosnexus_anvil::scripting::manager::PluginManager) {
    use std::io::Write;
    println!(
        "{}\t{}",
        chaosnexus_anvil::scripting::trace::SUPERVISED_TRACES_PREFIX,
        manager.traces_json()
    );
    let _ = std::io::stdout().flush();
}

/// Runs the engine as a long-lived supervised child process for ChaosNexus Forge.
///
/// Unlike the default MCP server mode, this does not occupy stdout with the
/// stdio transport. Instead it:
/// * boots the [`PluginManager`] (loading plugins, firing lifecycle hooks, and
///   starting timers/webhooks),
/// * forwards `mcp_log` output to structured stderr (plugin `log_*` already
///   mirrors itself via [`write_log`]),
/// * prints a `CHAOSFORGE_READY` marker to stdout on every ready transition,
/// * publishes the CVar snapshot (`dump_cvars`) on ready and after each change,
/// * streams trace snapshots (`dump_traces`) after each recorded span and on demand,
/// * and accepts `reload` / `stop` / `cvars` / `setcvar` / `savecvars` / `traces` line
///   commands on stdin so the supervisor can drive the engine without a restart.
async fn run_supervised(scripts_dir: &str, config: Arc<Config>) -> SdkResult<()> {
    use chaosnexus_anvil::scripting::manager::{PluginManager, emit_supervised_log};
    use chaosnexus_anvil::scripting::trace::set_supervised_trace_stream;
    use std::io::Write;
    use tokio::io::AsyncBufReadExt;

    set_supervised_trace_stream(true);

    let (log_tx, mut log_rx) = tokio::sync::mpsc::unbounded_channel::<(String, String, String)>();

    // Forward explicit mcp_log() output to structured stderr for the supervisor.
    tokio::spawn(async move {
        while let Some((plugin, level, msg)) = log_rx.recv().await {
            emit_supervised_log(&plugin, &level, &msg);
        }
    });

    let scripts_owned = scripts_dir.to_string();
    let plugins_dir = std::path::Path::new(scripts_dir).join("plugins");
    if !plugins_dir.exists() {
        let _ = std::fs::create_dir_all(&plugins_dir);
    }
    let mut manager = PluginManager::new(&scripts_owned, Some(log_tx.clone()), None, Arc::clone(&config));

    // Signal readiness so the supervisor can flip the status to "running", then
    // publish the initial CVar snapshot so the controller populates instantly.
    println!("CHAOSFORGE_READY");
    let _ = std::io::stdout().flush();
    dump_cvars(&manager);
    dump_traces(&manager);
    emit_supervised_log(
        "engine",
        "info",
        &format!("Supervised engine ready ({})", scripts_dir),
    );

    let stdin = tokio::io::BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        // Commands are tab-delimited: `<cmd>[\t<arg1>[\t<arg2>]]`.
        let trimmed = line.trim_end_matches(['\n', '\r']);
        let mut parts = trimmed.splitn(3, '\t');
        let cmd = parts.next().unwrap_or("").trim();

        match cmd {
            "reload" => {
                emit_supervised_log("engine", "info", "Reloading plugins...");
                manager.stop_all();
                manager = PluginManager::new(&scripts_owned, Some(log_tx.clone()), None, Arc::clone(&config));
                println!("CHAOSFORGE_READY");
                let _ = std::io::stdout().flush();
                dump_cvars(&manager);
                dump_traces(&manager);
                emit_supervised_log("engine", "info", "Plugins reloaded");
            }
            "cvars" => dump_cvars(&manager),
            "traces" => dump_traces(&manager),
            "setcvar" => {
                match (parts.next(), parts.next()) {
                    (Some(name), Some(value)) => match manager.set_cvar(name, value) {
                        Ok(msg) => emit_supervised_log("engine", "info", &msg),
                        Err(e) => emit_supervised_log("engine", "error", &e),
                    },
                    _ => emit_supervised_log("engine", "warn", "setcvar requires <name> <value>"),
                }
                dump_cvars(&manager);
            }
            "savecvars" => {
                match manager.persist_cvars() {
                    Ok(msg) => emit_supervised_log("engine", "info", &msg),
                    Err(e) => emit_supervised_log("engine", "error", &e),
                }
                dump_cvars(&manager);
            }
            "stop" => break,
            "" => {}
            other => emit_supervised_log("engine", "warn", &format!("Unknown command: {}", other)),
        }
    }

    // Graceful shutdown: stdin closed (EOF) or explicit "stop".
    manager.stop_all();
    emit_supervised_log("engine", "info", "Supervised engine stopped");
    Ok(())
}

/// Sets up a filesystem watcher to monitor for out-of-band events (like user approvals)
/// and relays them to the engine logs.
fn handle_file_events(
    events_dir: std::path::PathBuf,
    mcp_log_tx: tokio::sync::mpsc::UnboundedSender<(String, String, String)>,
) {
    use notify::{Event, EventKind, RecursiveMode, Watcher};
    let (tx, rx) = std::sync::mpsc::channel();
    let Ok(mut watcher) = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res
            && matches!(event.kind, EventKind::Create(_))
        {
            for path in event.paths {
                let _ = tx.send(path);
            }
        }
    }) else {
        return;
    };

    if watcher
        .watch(&events_dir, RecursiveMode::NonRecursive)
        .is_ok()
    {
        while let Ok(path) = rx.recv() {
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }

            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };

            let _ = std::fs::remove_file(&path);

            let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
                continue;
            };

            let Some(t) = json.get("type").and_then(|v| v.as_str()) else {
                continue;
            };
            if t != "plugin_status" {
                continue;
            }

            let Some(plugin_name) = json.get("plugin_name").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(status) = json.get("status").and_then(|v| v.as_str()) else {
                continue;
            };

            let msg = format!(
                "[SYSTEM: Plugin '{}' has been {} by the user]",
                plugin_name, status
            );
            let _ = mcp_log_tx.send(("system".to_string(), "info".to_string(), msg));
        }
    }
}

/// The main entry point for the ChaosNexus Anvil process.
#[tokio::main]
async fn main() -> SdkResult<()> {
    dotenvy::dotenv().ok();
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("panic: {}", info);
        if let Ok(path) = std::env::var("CHAOSWRENCH_DEBUG_LOG") {
            let line = format!("{} [chaosnexus-anvil] {}\n", Utc::now().to_rfc3339(), msg);
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| f.write_all(line.as_bytes()));
        } else {
            eprintln!("[chaosnexus-anvil] {}", msg);
        }
    }));

    debug_log("starting");
    let args = Cli::parse();

    // Emit the schema to stdout for build pipelines (docs generator). This is
    // checked first so it never touches the OS config directory.
    if args.schema_stdout {
        print!(
            "{}",
            chaosnexus_anvil::scripting::engine::generate_system_schema()
        );
        return Ok(());
    }

    // Emit function signatures for the visual-scripting canvas. Checked before
    // config/transport setup so it stays a fast, side-effect-free one-shot.
    if let Some(path) = args.list_functions.as_deref() {
        match std::fs::read_to_string(path) {
            Ok(source) => {
                print!(
                    "{}",
                    chaosnexus_anvil::scripting::graph::manifest::signatures_json(&source)
                );
                return Ok(());
            }
            Err(e) => {
                eprintln!("Failed to read {}: {}", path, e);
                std::process::exit(1);
            }
        }
    }

    if args.generate_schema {
        let schema_json = chaosnexus_anvil::scripting::engine::generate_system_schema();

        let Some(proj_dirs) = directories::ProjectDirs::from("com", "tunedchaos", "chaosnexus-forge") else {
            eprintln!("Could not determine OS config directory");
            std::process::exit(1);
        };
        
        let config_dir = proj_dirs.config_dir();
        if !config_dir.exists() {
            let _ = std::fs::create_dir_all(config_dir);
        }
        let schema_path = config_dir.join("chaos_schema.json");
        if let Err(e) = std::fs::write(&schema_path, schema_json) {
            eprintln!("Failed to write schema to {:?}: {}", schema_path, e);
            std::process::exit(1);
        }
        
        println!("Successfully generated schema at {:?}", schema_path);
        return Ok(());
    }

    let mut base_config = Config::load_base(args.config.as_deref()).unwrap_or_else(|e| {
        debug_log(&format!("config load error: {}", e));
        eprintln!("[chaosnexus-anvil] config load error: {}", e);
        Config::default()
    });
    debug_log("base config loaded");

    let instances_dir = directories::BaseDirs::new()
        .expect("Failed to get base dirs")
        .home_dir()
        .join(".chaosnexus")
        .join("chaosnexus-anvil")
        .join("instances");
    let _ = std::fs::create_dir_all(&instances_dir);

    let pid = std::process::id();
    let instance_file = instances_dir.join(format!("instance_{}.json", pid));

    let parent_name = std::env::var("CHAOSWRENCH_PARENT").unwrap_or_else(|_| "unknown".to_string());
    
    let instance_name = args
        .name
        .clone()
        .or_else(|| base_config.name.clone())
        .unwrap_or_else(|| {
            let mut max_id = 0;
            if let Ok(entries) = std::fs::read_dir(&instances_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !path.is_file() {
                        continue;
                    }
                    let Ok(contents) = std::fs::read_to_string(&path) else { continue; };
                    let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) else { continue; };
                    let Some(name) = json.get("name").and_then(|n| n.as_str()) else { continue; };
                    let Some(num_str) = name.strip_prefix("Instance-") else { continue; };
                    let Ok(num) = num_str.parse::<u32>() else { continue; };
                    max_id = max_id.max(num);
                }
            }
            format!("Instance-{}", max_id + 1)
        });

    unsafe {
        std::env::set_var("CHAOS_INSTANCE_NAME", &instance_name);
    }

    base_config.load_instance_override(&instance_name);
    let config = Arc::new(base_config);

    // `--scripts-dir` (used by ChaosNexus Forge) takes precedence over the config file.
    let scripts_dir_opt = args
        .scripts_dir
        .as_deref()
        .or(config.scripts_dir.as_deref());

    let default_scripts_dir = directories::BaseDirs::new()
        .expect("Failed to get base dirs")
        .home_dir()
        .join(".chaosnexus")
        .join("chaosnexus-anvil")
        .join(&instance_name)
        .join("scripts");

    let scripts_dir_path = scripts_dir_opt
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            if std::path::Path::new("../chaosnexus-scripts").exists() {
                std::path::PathBuf::from("../chaosnexus-scripts")
            } else if std::path::Path::new("./chaosnexus-scripts").exists() {
                std::path::PathBuf::from("./chaosnexus-scripts")
            } else {
                default_scripts_dir
            }
        });
        
    let scripts_dir_owned = scripts_dir_path.to_string_lossy().to_string();
    let scripts_dir = &scripts_dir_owned;

    chaosnexus_anvil::scripting::sandbox::warn_if_privileged();
    chaosnexus_anvil::scripting::sandbox::apply_filesystem_sandbox_if_available(std::path::Path::new(
        scripts_dir,
    ));

    // Supervised mode for ChaosNexus Forge runs a long-lived engine without the MCP
    if args.supervised {
        return run_supervised(scripts_dir, Arc::clone(&config)).await;
    }

    let (mcp_log_tx, mut mcp_log_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, String, String)>();

    let mcp_log_tx_clone = mcp_log_tx.clone();
    let events_dir = std::path::PathBuf::from(scripts_dir)
        .join(".chaoswrench_data")
        .join("events");
    if !events_dir.exists() {
        let _ = std::fs::create_dir_all(&events_dir);
    }

    tokio::task::spawn_blocking(move || {
        handle_file_events(events_dir, mcp_log_tx_clone);
    });

    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(std::process::id().to_string().as_bytes());
    hasher.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .to_string()
            .as_bytes(),
    );
    let token = hex::encode(hasher.finalize());

    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind random port for SSE");
    let sse_port = listener
        .local_addr()
        .expect("Failed to get local addr")
        .port();
    drop(listener);

    let plugin_manager = std::sync::Arc::new(tokio::sync::RwLock::new(
        chaosnexus_anvil::scripting::manager::PluginManager::new(
            scripts_dir,
            Some(mcp_log_tx),
            Some((sse_port, token.clone())),
            Arc::clone(&config),
        ),
    ));

    let pm_clone = plugin_manager.clone();
    let plugins_watch_dir = std::path::PathBuf::from(scripts_dir).join("plugins");
    if !plugins_watch_dir.exists() {
        let _ = std::fs::create_dir_all(&plugins_watch_dir);
    }

    let server_details = InitializeResult {
        server_info: Implementation {
            name: "chaosnexus-anvil".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            title: Some("ChaosNexus Anvil MCP Server".to_string()),
            description: Some("A sandboxed, plugin-driven Rhai scripting engine. Plugins are quarantined until a human approves them, then auto-loaded and exposed as MCP tools.".to_string()),
            icons: vec![],
            website_url: None,
        },
        capabilities: ServerCapabilities {
            tools: Some(ServerCapabilitiesTools { list_changed: Some(true) }),
            logging: Some(serde_json::Map::new()),
            ..Default::default()
        },
        meta: None,
        protocol_version: ProtocolVersion::V2025_11_25.into(),
        instructions: Some(
            "ChaosNexus Anvil is a sandboxed Rhai scripting engine. \
             You can create plugins with `create_plugin` (they are quarantined until a human approves them), \
             disable live plugins with `disable_plugin`, \
             check plugin approval status with `check_plugin_status`, \
             search the Rhai language documentation with `query_rhai_docs`, \
             and inspect or update runtime configuration with `cvars`. \
             All Rhai scripts run inside a strict sandbox with no filesystem or network access unless explicitly granted by the user.".to_string(),
        ),
    };

    let transport = StdioTransport::new(TransportOptions::default())?;
    debug_log("stdio transport ready");

    struct ProxyClientHandler;
    #[async_trait::async_trait]
    impl rust_mcp_sdk::mcp_client::ClientHandler for ProxyClientHandler {}

    let mut proxy_clients_map = std::collections::HashMap::new();
    if let Some(mcp) = &config.mcp_servers {
        for (name, srv_cfg) in mcp {
            let client_details = rust_mcp_sdk::schema::InitializeRequestParams {
                capabilities: rust_mcp_sdk::schema::ClientCapabilities::default(),
                client_info: rust_mcp_sdk::schema::Implementation {
                    name: "chaosnexus-anvil-proxy".into(),
                    version: "0.1.0".into(),
                    description: None,
                    icons: vec![],
                    title: None,
                    website_url: None,
                },
                protocol_version: rust_mcp_sdk::schema::ProtocolVersion::V2025_11_25.into(),
                meta: None,
            };

            let prefix = srv_cfg.prefix.clone().unwrap_or_else(|| name.clone());
            let transport = match rust_mcp_transport::StdioTransport::create_with_server_launch(
                &srv_cfg.command,
                srv_cfg.args.clone(),
                None,
                rust_mcp_transport::TransportOptions::default(),
            ) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("[chaosnexus-anvil] failed to launch proxy server {}: {}", name, e);
                    continue;
                }
            };

            use rust_mcp_sdk::mcp_client::ToMcpClientHandler;
            let client = rust_mcp_sdk::mcp_client::client_runtime::create_client(rust_mcp_sdk::mcp_client::McpClientOptions {
                client_details,
                transport,
                handler: ProxyClientHandler.to_mcp_client_handler(),
                task_store: None,
                server_task_store: None,
                message_observer: None,
            });

            use rust_mcp_sdk::McpClient;
            if let Err(e) = client.clone().start().await {
                eprintln!("[chaosnexus-anvil] failed to start proxy server {}: {}", name, e);
                continue;
            }

            debug_log(&format!("Started proxy server {}", name));
            proxy_clients_map.insert(prefix, client);
        }
    }

    let proxy_clients = std::sync::Arc::new(tokio::sync::RwLock::new(proxy_clients_map));

    let handler = Handler {
        plugin_manager: plugin_manager.clone(),
        proxy_clients: proxy_clients.clone(),
        max_proxy_response_length: config.max_proxy_response_length.unwrap_or(16384),
    }
    .to_mcp_server_handler();

    let server = server_runtime::create_server(McpServerOptions {
        server_details: server_details.clone(),
        transport,
        handler: handler.clone(),
        task_store: None,
        client_task_store: None,
        message_observer: None,
    });

    let sse_handler = Handler {
        plugin_manager: plugin_manager.clone(),
        proxy_clients: proxy_clients.clone(),
        max_proxy_response_length: config.max_proxy_response_length.unwrap_or(16384),
    }
    .to_mcp_server_handler();

    let server_options = rust_mcp_axum::AxumServerOptions {
        host: "127.0.0.1".to_string(),
        port: sse_port,
        ..Default::default()
    };

    let axum_server =
        rust_mcp_axum::create_axum_server(server_details, sse_handler, server_options);

    let _sse_runtime = Arc::new(
        axum_server
            .start_runtime()
            .await
            .expect("Failed to start SSE server"),
    );

    let (log_tx, mut log_rx) = tokio::sync::broadcast::channel(1024);
    chaosnexus_anvil::scripting::manager::set_sse_log_sender(log_tx);

    let sse_runtime_for_logs = Arc::clone(&_sse_runtime);
    tokio::spawn(async move {
        while let Ok(msg) = log_rx.recv().await {
            if let rust_mcp_sdk::schema::ServerNotification::LoggingMessageNotification(notif) = msg
            {
                let sessions = sse_runtime_for_logs.sessions().await;
                for session_id in sessions {
                    let _ = sse_runtime_for_logs
                        .notify_log_message(&session_id, notif.params.clone())
                        .await;
                }
            }
        }
    });


        
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let discovery_data = serde_json::json!({
        "pid": pid,
        "name": instance_name,
        "parent": parent_name,
        "port": sse_port,
        "token": token,
        "timestamp": timestamp
    });

    let _ = std::fs::write(
        &instance_file,
        serde_json::to_string_pretty(&discovery_data).unwrap(),
    );

    let server_clone_for_watcher = server.clone();
    tokio::spawn(async move {
        use notify::{Event, EventKind, RecursiveMode, Watcher};
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let Ok(mut watcher) = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                        let _ = tx.send(());
                    }
                    _ => {}
                }
            }
        }) else {
            return;
        };

        if !plugins_watch_dir.exists() {
            std::fs::create_dir_all(&plugins_watch_dir).ok();
        }

        if watcher
            .watch(&plugins_watch_dir, RecursiveMode::Recursive)
            .is_ok()
        {
            loop {
                // Wait for the first event
                if rx.recv().await.is_none() {
                    break;
                }

                // Debounce: wait 500ms, consuming any subsequent events
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                while rx.try_recv().is_ok() {}

                let mut pm = pm_clone.write().await;
                pm.rebuild_from_disk();
                let _ = server_clone_for_watcher
                    .notify_tool_list_changed(None)
                    .await;
            }
        }
    });

    let server_clone = server.clone();
    tokio::spawn(async move {
        while let Some((plugin_name, level, msg)) = mcp_log_rx.recv().await {
            let mcp_level = match level.to_lowercase().as_str() {
                "debug" => rust_mcp_sdk::schema::LoggingLevel::Debug,
                "info" => rust_mcp_sdk::schema::LoggingLevel::Info,
                "warn" => rust_mcp_sdk::schema::LoggingLevel::Warning,
                "error" => rust_mcp_sdk::schema::LoggingLevel::Error,
                _ => rust_mcp_sdk::schema::LoggingLevel::Info,
            };
            let _ = server_clone
                .notify_log_message(rust_mcp_sdk::schema::LoggingMessageNotificationParams {
                    level: mcp_level,
                    logger: Some(plugin_name),
                    data: serde_json::Value::String(msg),
                    meta: None,
                })
                .await;
        }
    });

    debug_log("entering main loop (waiting for MCP on stdin/stdout)");
    let result = server.start().await;

    if let Err(ref e) = result {
        debug_log(&format!("server.start() exited with error: {}", e));
    } else {
        debug_log("server.start() returned Ok (transport closed)");
    }

    let _ = std::fs::remove_file(&instance_file);

    result
}
