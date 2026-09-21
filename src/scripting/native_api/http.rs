// chaosnexus-anvil/src/scripting/native_api/http.rs
use crate::scripting::capabilities::Capability;
use crate::scripting::models::NativeContext;
use crate::scripting::native_api::gates::require_cap;
use crate::scripting::utils::run_async;
use rhai::Engine;

use crate::scripting::manager::write_log;
use std::sync::Arc;

fn net_star_allowed_by_env() -> bool {
    for key in [
        "CHAOSNEXUS_ANVIL_ALLOW_NET_STAR",
        "CHAOSWRENCH_ALLOW_NET_STAR",
    ] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim();
            if v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes") {
                return true;
            }
        }
    }
    false
}

fn http_method_star_allowed_by_env() -> bool {
    for key in [
        "CHAOSNEXUS_ANVIL_ALLOW_HTTP_METHOD_STAR",
        "CHAOSWRENCH_ALLOW_HTTP_METHOD_STAR",
    ] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim();
            if v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes") {
                return true;
            }
        }
    }
    false
}

/// Extract hostname (lowercase) from a URL or host:port string. No substring tricks.
fn extract_host(url_or_addr: &str) -> Option<String> {
    let s = url_or_addr.trim();
    if let Ok(parsed) = reqwest::Url::parse(s) {
        return parsed.host_str().map(|h| h.to_lowercase());
    }
    // Bare host or host:port (tcp_request / ntp)
    let no_path = s.split('/').next().unwrap_or(s);
    let host = no_path.split(':').next().unwrap_or(no_path).trim();
    if host.is_empty() {
        None
    } else {
        Some(host.to_lowercase())
    }
}

/// Helper for domain wildcard matching (`*.github.com` or `api.github.com`).
/// Bare `*` is deny-by-default; set CHAOSNEXUS_ANVIL_ALLOW_NET_STAR=1 to opt in.
/// Matching is host-only (never `url.contains`).
fn match_domain(url_str: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return net_star_allowed_by_env();
    }
    let Some(host) = extract_host(url_str) else {
        return false;
    };
    let pat = pattern.trim().to_lowercase();

    if let Some(with_dot) = pat.strip_prefix('*') {
        host.ends_with(with_dot)
            || with_dot
                .strip_prefix('.')
                .is_some_and(|domain| host == domain)
    } else {
        host == pat
    }
}

fn is_blocked_destination_host(host: &str) -> bool {
    // Basic SSRF guard for literal IPs / localhost after allowlist match.
    let h = host.trim_matches(|c| c == '[' || c == ']').to_lowercase();
    if h == "localhost" || h.ends_with(".localhost") || h == "0.0.0.0" {
        return true;
    }
    if let Ok(ip) = h.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => {
                v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
            }
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local() || v6.is_unspecified()
            }
        };
    }
    false
}

/// Shared HTTP client: never follow redirects (prevents allowlist bypass via 302).
fn http_client_no_redirects() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Verifies whether the calling plugin is permitted to access a specific network URL and method.
pub(crate) fn verify_network_access(ctx: &NativeContext, url: &str, method: &str) -> Result<(), Box<rhai::EvalAltResult>> {
    let plugin_name = crate::scripting::plugin_context::current_plugin()
        .unwrap_or_else(|| "unknown".to_string());

    let perms = ctx.plugins.read().unwrap();
    let plugin_config = perms.get(&plugin_name);

    if let Some(config) = plugin_config
        && let Some(permissions) = &config.permissions {
        let allowlist = permissions.net_allowlist.as_ref().or(permissions.http_domains.as_ref());
        let domain_allowed = allowlist.is_some_and(|domains| domains.iter().any(|d| match_domain(url, d)));
        
        // TCP/UDP/WS/NTP are transport checks: domain allowlist alone (cap already required).
        let transport_only = matches!(
            method.to_ascii_uppercase().as_str(),
            "TCP" | "UDP" | "WS" | "NTP"
        );
        let method_allowed = transport_only
            || permissions.http.as_ref().is_some_and(|methods| {
                methods.iter().any(|m| {
                    if m == "*" {
                        http_method_star_allowed_by_env()
                    } else {
                        m.eq_ignore_ascii_case(method)
                    }
                })
            });

        if domain_allowed && method_allowed {
            // Extra SSRF guard: reject literal private/loopback hosts unless net star opted in.
            if let Some(host) = extract_host(url) {
                if is_blocked_destination_host(&host) && !net_star_allowed_by_env() {
                    return Err(format!(
                        "Security Violation: destination host '{host}' is blocked (private/loopback).                          Set CHAOSNEXUS_ANVIL_ALLOW_NET_STAR=1 only if you intentionally need it."
                    )
                    .into());
                }
            }
            return Ok(());
        }
    }
    Err(format!("Security Violation: Network access to '{}' using method '{}' is not explicitly allowed in chaosnexus-anvil.toml for plugin '{}'. Deny-by-default is in effect.", url, method, plugin_name).into())
}

/// Registers HTTP and WebSocket native functions with the Rhai engine.
pub fn register(engine: &mut Engine, n_ctx: &NativeContext) {
    let ctx = n_ctx.clone();
    engine.register_fn("ws_connect", move |url: &str, callback: &str| -> Result<(), Box<rhai::EvalAltResult>> {
            let plugin_name = crate::scripting::plugin_context::current_plugin_name();
            let plugin_name = plugin_name.as_str();
            require_cap(&ctx, Capability::NetWs)?;
            verify_network_access(&ctx, url, "WS")?;
            let (kill_tx, mut kill_rx) = tokio::sync::mpsc::channel::<()>(1);
            let mut handles = ctx.ws_handles.lock().unwrap();
            handles.insert(url.to_string(), kill_tx);

            let url_str = url.to_string();
            let p_name = plugin_name.to_string();
            let cb_name = callback.to_string();
            let a = ctx.asts.clone();

            tokio::spawn(async move {
                use futures_util::StreamExt;
                let request = match tokio_tungstenite::tungstenite::http::Request::builder()
                    .uri(&url_str)
                    .body(()) {
                        Ok(req) => req,
                        Err(e) => {
                            write_log(&p_name, "ERROR", &format!("WS URI error: {}", e));
                            return;
                        }
                    };

                let (ws_stream, _) = match tokio_tungstenite::connect_async(request).await {
                    Ok(res) => res,
                    Err(e) => {
                        write_log(&p_name, "ERROR", &format!("WS Connect failed: {}", e));
                        return;
                    }
                };

                write_log(&p_name, "INFO", &format!("Connected to WS: {}", url_str));
                let (_, mut read) = ws_stream.split();

                loop {
                    tokio::select! {
                        msg = read.next() => {
                            match msg {
                                Some(Ok(m)) => {
                                    let Ok(text) = m.into_text() else {
                                        continue;
                                    };
                                    let target_ast = {
                                        let asts_guard = a.lock().unwrap();
                                        asts_guard.get(&p_name).cloned()
                                    };
                                    let Some(ast) = target_ast else {
                                        break; // Plugin unloaded
                                    };
                                    let cb_clone = cb_name.clone();
                                    let p_name_clone = p_name.clone();
                                    let _ = tokio::task::spawn_blocking(move || {
                                        let temp_engine = rhai::Engine::new();
                                        let mut scope = rhai::Scope::new();
                                        scope.push("PLUGIN_NAME", p_name_clone);
                                        let _ = temp_engine.call_fn::<()>(&mut scope, &ast, &cb_clone, (text,));
                                    }).await;
                                }
                                Some(Err(e)) => {
                                    write_log(&p_name, "ERROR", &format!("WS Error: {}", e));
                                    break;
                                }
                                None => break, // closed
                            }
                        }
                        _ = kill_rx.recv() => {
                            write_log(&p_name, "INFO", &format!("WS connection closed by ws_close: {}", url_str));
                            break;
                        }
                    }
                }
            });
            Ok(())
        });

    let ctx = n_ctx.clone();
    engine.register_fn("ws_close", move |url: &str| {
        let mut handles = ctx.ws_handles.lock().unwrap();
        if let Some(kill_tx) = handles.remove(url) {
            let _ = kill_tx.try_send(());
        }
    });

    let ctx = n_ctx.clone();
    engine.register_fn(
        "route_webhook",
        move |port: i64, path: &str, callback: &str| -> Result<(), Box<rhai::EvalAltResult>> {
            require_cap(&ctx, Capability::NetHttp)?;
            let plugin_name = crate::scripting::plugin_context::current_plugin_name();
            let plugin_name = plugin_name.as_str();
            let mut routes = ctx.webhook_routes.write().unwrap();
            routes.entry(port).or_default().insert(
                path.to_string(),
                (plugin_name.to_string(), callback.to_string()),
            );
            Ok(())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn("start_webhook_server", move |port: i64| -> Result<(), Box<rhai::EvalAltResult>> {
            require_cap(&ctx, Capability::NetHttp)?;
            if !(1..=65535).contains(&port) {
                return Err("Invalid webhook port".into());
            }
            let plugin_name = crate::scripting::plugin_context::current_plugin_name();
            let plugin_name = plugin_name.as_str();
            let mut handles = ctx.webhook_handles.lock().unwrap();
            if handles.contains_key(&port) {
                return Ok(()); // Already running
            }

            let (kill_tx, mut kill_rx) = tokio::sync::mpsc::channel::<()>(1);
            handles.insert(port, kill_tx);

            let routes_read = Arc::clone(&ctx.webhook_routes);
            let asts_read = Arc::clone(&ctx.asts);
            let p_name = plugin_name.to_string();

            tokio::spawn(async move {
                use axum::{Router, extract::Request, response::IntoResponse};

                let app = Router::new().fallback(
                    move |req: Request| async move {
                        let path = req.uri().path().to_string();
                        let method = req.method().clone();

                        let bytes = match axum::body::to_bytes(req.into_body(), 1_048_576).await {
                            Ok(b) => b,
                            Err(_) => return axum::http::StatusCode::BAD_REQUEST.into_response(),
                        };
                        let body_str = String::from_utf8_lossy(&bytes).to_string();

                        let route_info = routes_read.read().unwrap().get(&port).and_then(|pr| pr.get(&path).cloned());

                        let Some((target_plugin, callback)) = route_info else {
                            return axum::http::StatusCode::NOT_FOUND.into_response();
                        };

                        let target_ast = asts_read.lock().unwrap().get(&target_plugin).cloned();
                        let Some(ast) = target_ast else {
                            return axum::http::StatusCode::NOT_FOUND.into_response();
                        };

                        let _ = tokio::task::spawn_blocking(move || {
                            let temp_engine = rhai::Engine::new();
                            let mut scope = rhai::Scope::new();
                            scope.push("PLUGIN_NAME", target_plugin);
                            let mut map = rhai::Map::new();
                            map.insert("method".into(), rhai::Dynamic::from(method.as_str().to_string()));
                            map.insert("path".into(), rhai::Dynamic::from(path));
                            map.insert("body".into(), rhai::Dynamic::from(body_str));

                            let _ = temp_engine.call_fn::<()>(&mut scope, &ast, &callback, (map,));
                        }).await;
                        axum::http::StatusCode::OK.into_response()
                    }
                );

                let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port as u16));
                let listener = match tokio::net::TcpListener::bind(&addr).await {
                    Ok(l) => l,
                    Err(e) => {
                        write_log(&p_name, "ERROR", &format!("Webhook bind failed: {}", e));
                        return;
                    }
                };

                write_log(&p_name, "INFO", &format!("Webhook Server listening on {}", addr));

                tokio::select! {
                    _ = axum::serve(listener, app).into_future() => {}
                    _ = kill_rx.recv() => {
                        write_log(&p_name, "INFO", &format!("Webhook Server on {} shut down.", port));
                    }
                }
            });

            Ok(())
        });
    // NOTE: a second `http_get` (blocking reqwest client) previously registered
    // here was always shadowed by the async registration below (same name and
    // arity), so it was dead code and has been removed.
    let ctx = n_ctx.clone();
    engine.register_fn(
        "http_get",
        move |url: &str| -> Result<String, Box<rhai::EvalAltResult>> {
            require_cap(&ctx, Capability::NetHttp)?;
            verify_network_access(&ctx, url, "GET")?;
            let url_owned = url.to_string();
            let res: Result<String, reqwest::Error> = run_async(async move {
                let client = http_client_no_redirects();
                client.get(&url_owned).send().await?.text().await
            });
            res.map_err(|e| format!("http_get error: {}", e).into())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "http_post",
        move |url: &str, body: &str| -> Result<String, Box<rhai::EvalAltResult>> {
            require_cap(&ctx, Capability::NetHttp)?;
            verify_network_access(&ctx, url, "POST")?;
            let url_owned = url.to_string();
            let body_owned = body.to_string();
            let res: Result<String, reqwest::Error> = run_async(async move {
                let client = http_client_no_redirects();
                client
                    .post(&url_owned)
                    .body(body_owned)
                    .send()
                    .await?
                    .text()
                    .await
            });
            res.map_err(|e| format!("http_post error: {}", e).into())
        },
    );

    let ctx = n_ctx.clone();
    engine.register_fn(
        "tcp_request",
        move |address: &str, data: &str| -> Result<String, Box<rhai::EvalAltResult>> {
            require_cap(&ctx, Capability::NetTcp)?;
            verify_network_access(&ctx, address, "TCP")?;
            let addr_owned = address.to_string();
            let data_owned = data.to_string();
            let res: Result<String, std::io::Error> = run_async(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut stream = tokio::net::TcpStream::connect(&addr_owned).await?;
                stream.write_all(data_owned.as_bytes()).await?;
                let mut temp = vec![0; 4096];
                let n = stream.read(&mut temp).await?;
                let buf = String::from_utf8_lossy(&temp[..n]).to_string();
                Ok(buf)
            });
            res.map_err(|e| format!("tcp_request error: {}", e).into())
        },
    );
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_star_denied_by_default() {
        unsafe {
            std::env::remove_var("CHAOSNEXUS_ANVIL_ALLOW_NET_STAR");
            std::env::remove_var("CHAOSWRENCH_ALLOW_NET_STAR");
        }
        assert!(!match_domain("https://evil.example", "*"));
    }

    #[test]
    fn match_domain_is_host_only_not_substring() {
        assert!(match_domain("https://api.github.com/foo", "api.github.com"));
        assert!(!match_domain("https://notevil.com/x", "evil.com"));
        assert!(!match_domain("https://attacker.example/?q=api.github.com", "api.github.com"));
        assert!(match_domain("https://foo.github.com/bar", "*.github.com"));
    }

    #[test]
    fn blocked_private_hosts() {
        assert!(is_blocked_destination_host("127.0.0.1"));
        assert!(is_blocked_destination_host("localhost"));
        assert!(is_blocked_destination_host("10.0.0.1"));
        assert!(!is_blocked_destination_host("api.github.com"));
    }
}
