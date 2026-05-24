//! # EasyTex — Server Entrypoint
//!
//! EasyTex is a high-performance, resilient LaTeX preview server engineered using Axum.
//! This module acts as the CLI parser and server bootloader, supporting:
//!
//! * Port and host bindings configured dynamically via config yaml, flags, or system environment.
//! * Automatic diagnostics checks (`diag` subcommand) verifying filesystem permissions, port availability, and LaTeX tooling path existence.
//! * Dynamic runtime capabilities routing.
//! * Graceful OS interrupt signal processing (SIGTERM, SIGINT) ensuring clean cancellation of active LaTeX compiler subprocesses.

mod artifacts;
mod builder;
mod capabilities;
mod config;
mod dto;
mod errors;
mod events;
mod frontend_assets;
mod fs_safety;
mod handlers;
mod process;
mod state;
mod utils;

use anyhow::{Context, Result};
use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, Method, Request},
    middleware::{self, Next},
    response::Response,
    routing::{any, get},
    Router,
};
use clap::{Parser, Subcommand};
use std::{
    collections::HashMap,
    net::TcpListener as StdTcpListener,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, info, trace, warn};

use crate::capabilities::{RuntimeCapabilities, OPTIONAL_TOOLS};
use crate::config::GlobalConfig;
use crate::frontend_assets::FrontendAssets;
use crate::handlers::{
    admin_api_handler, admin_dashboard, admin_metrics, api_handler, api_handler_no_name, get_pdf,
    sse_handler, static_handler,
};
use crate::state::{cleanup_expired_sessions, load_build_history, AppState};
use crate::utils::{command_exists, is_valid_project_name};

/// Globally shared thread-safe counter tracking total HTTP requests received for diagnostics correlation.
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

fn redacted_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            let name_str = name.as_str();
            let lower = name_str.to_ascii_lowercase();
            let is_sensitive = matches!(
                lower.as_str(),
                "authorization"
                    | "cookie"
                    | "set-cookie"
                    | "proxy-authorization"
                    | "x-api-key"
                    | "x-auth-token"
                    | "x-easytex-admin-token"
            ) || lower.contains("token")
                || lower.contains("secret")
                || lower.contains("password");
            let value = if is_sensitive {
                "[REDACTED]".to_string()
            } else {
                value
                    .to_str()
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|_| "[NON_UTF8]".to_string())
            };
            (name_str.to_string(), value)
        })
        .collect()
}

/// HTTP middleware capturing request telemetry, duration, status codes, and user agents.
///
/// Automatically logs warnings on high-latency routes (>500ms) or client/server error responses.
async fn logging_middleware(req: Request<Body>, next: Next) -> Response {
    let request_id = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let method = req.method().clone();
    let uri = req.uri().clone();
    let path = uri.path().to_string();
    let user_agent = req
        .headers()
        .get(axum::http::header::USER_AGENT)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("-");
    let start = Instant::now();

    info!(
        request_id,
        method = %method,
        path = %path,
        "HTTP request started"
    );
    trace!(
        request_id,
        method = %method,
        uri = %uri,
        user_agent = %user_agent,
        headers = ?redacted_headers(req.headers()),
        "HTTP request details"
    );

    let response = next.run(req).await;
    let duration = start.elapsed();
    let status = response.status();

    if status.is_server_error() {
        warn!(
            request_id,
            method = %method,
            uri = %uri,
            status = %status,
            duration_ms = duration.as_millis(),
            "HTTP request failed"
        );
    } else if status.is_client_error() {
        warn!(
            request_id,
            method = %method,
            uri = %uri,
            status = %status,
            duration_ms = duration.as_millis(),
            "HTTP request rejected"
        );
    } else {
        info!(
            request_id,
            method = %method,
            uri = %uri,
            status = %status,
            duration_ms = duration.as_millis(),
            "HTTP request completed"
        );
    }

    if duration.as_millis() > 500 {
        debug!(
            request_id,
            method = %method,
            uri = %uri,
            status = %status,
            duration_ms = duration.as_millis(),
            "Slow HTTP request"
        );
    } else {
        trace!(
            request_id,
            method = %method,
            uri = %uri,
            status = %status,
            duration_ms = duration.as_millis(),
            response_headers = ?redacted_headers(response.headers()),
            "HTTP response details"
        );
    }

    response
}

/// Command-Line Interface parser for the EasyTex compiler server.
#[derive(Parser)]
#[command(name = "easytex")]
#[command(version = "0.2.0")]
#[command(about = "EasyTex — High-performance LaTeX Preview Server", long_about = None)]
struct Cli {
    /// Optional subcommand target. If not provided, defaults to Serve.
    #[command(subcommand)]
    command: Option<Commands>,
}

/// Supported EasyTex operational subcommands.
#[derive(Subcommand)]
enum Commands {
    /// Start the real-time preview compilation and dashboard server.
    Serve {
        /// Parent directory storing all sandboxed LaTeX projects.
        #[arg(default_value = ".")]
        root: String,

        /// Custom port to bind the server socket listener to.
        #[arg(short, long)]
        port: Option<u16>,

        /// Network interface or host IP address to bind the listener socket.
        #[arg(long)]
        host: Option<String>,

        /// Relative or absolute path to the main YAML configuration file.
        #[arg(short, long, default_value = "easytex.yaml")]
        config: String,
    },
    /// Perform dynamic diagnoses checking tooling path, port availability, and folder permissions.
    #[command(alias = "diagnostic", alias = "diagonostic")]
    Diag {
        /// Project parent root directory.
        #[arg(default_value = ".")]
        root: String,

        /// Network port to verify for socket binding eligibility.
        #[arg(short, long)]
        port: Option<u16>,

        /// Host IP interface to query for socket binding eligibility.
        #[arg(long)]
        host: Option<String>,

        /// System configuration file location.
        #[arg(short, long, default_value = "easytex.yaml")]
        config: String,
    },
    /// Initialize a standard templated sandboxed project in the local workspace directory.
    Init {
        /// Unique safe project name.
        name: String,
    },
    /// Print EasyTex version information.
    Version {
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Serve {
            root,
            port,
            host,
            config,
        }) => {
            run_server(root, port, host, config).await?;
        }
        Some(Commands::Diag {
            root,
            port,
            host,
            config,
        }) => {
            let ok = run_diagnostics(root, port, host, config).await?;
            if !ok {
                std::process::exit(1);
            }
        }
        Some(Commands::Init { name }) => {
            if !is_valid_project_name(&name) {
                println!("Invalid project name: {}", name);
                std::process::exit(1);
            }
            let p = std::path::PathBuf::from(&name);
            if p.exists() {
                println!("Directory already exists: {}", name);
                std::process::exit(1);
            }
            std::fs::create_dir_all(&p)?;
            std::fs::write(p.join("EasyTex.toml"), "entrypoint = \"main.tex\"\n")?;
            std::fs::write(
                p.join("main.tex"),
                "\\documentclass{article}\n\\begin{document}\nHello EasyTex!\n\\end{document}",
            )?;
            println!("Project '{}' initialized successfully.", name);
        }
        Some(Commands::Version { json }) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "name": env!("CARGO_PKG_NAME"),
                        "version": env!("CARGO_PKG_VERSION"),
                        "features": {
                            "latexmk": cfg!(feature = "latexmk")
                        }
                    })
                );
            } else {
                println!("easytex {}", env!("CARGO_PKG_VERSION"));
            }
        }
        None => {
            run_server(".".into(), None, None, "easytex.yaml".into()).await?;
        }
    }
    Ok(())
}

fn check_runtime_dependencies(compress_pdf: bool) -> Result<()> {
    #[cfg(feature = "latexmk")]
    let required_deps = ["latexmk", "texfot"];

    #[cfg(not(feature = "latexmk"))]
    let required_deps = ["tectonic"];

    let mut missing = Vec::new();
    for dep in required_deps {
        if !command_exists(dep) {
            missing.push(dep);
        }
    }
    if !missing.is_empty() {
        anyhow::bail!(
            "Missing required build dependencies: {}. Please ensure they are installed and in your PATH.",
            missing.join(", ")
        );
    }

    for tool in OPTIONAL_TOOLS {
        if tool.command == "gs" && !compress_pdf {
            continue;
        }
        #[cfg(not(feature = "latexmk"))]
        if tool.command == "texloganalyser" {
            continue;
        }
        if command_exists(tool.command) {
            continue;
        }
        warn!(
            "Optional runtime tool '{}' unavailable; {} is disabled until it is installed and available in PATH.",
            tool.command, tool.description
        );
    }
    Ok(())
}

fn cors_layer(cfg: &GlobalConfig) -> CorsLayer {
    let layer = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            header::HeaderName::from_static("x-easytex-request"),
        ]);

    if cfg
        .cors_allowed_origins
        .iter()
        .any(|origin| origin.trim() == "*")
    {
        return layer.allow_origin(Any);
    }

    let origins = cfg
        .cors_allowed_origins
        .iter()
        .filter_map(|origin| origin.parse::<HeaderValue>().ok())
        .collect::<Vec<_>>();

    if origins.is_empty() {
        layer
    } else {
        layer.allow_origin(origins)
    }
}

fn load_global_config(config_path: &str) -> Result<GlobalConfig> {
    let cfg_path = std::path::PathBuf::from(config_path);
    let g_cfg: GlobalConfig = if cfg_path.exists() {
        let content = std::fs::read_to_string(&cfg_path)?;
        serde_yaml::from_str(&content).context("Failed to parse config file")?
    } else {
        let d = GlobalConfig::default();
        let content = serde_yaml::to_string(&d)?;
        let _ = std::fs::write(&cfg_path, content);
        d
    }
    .apply_env();
    g_cfg.validate()?;
    Ok(g_cfg)
}

fn history_path(root: &std::path::Path, history_file: &str) -> PathBuf {
    let configured = PathBuf::from(history_file);
    if configured.is_absolute() {
        configured
    } else {
        root.join(configured)
    }
}

fn resolve_root(root_str: &str, cfg: &GlobalConfig) -> Result<PathBuf> {
    let requested_root = if root_str == "." {
        &cfg.root_dir
    } else {
        root_str
    };
    PathBuf::from(requested_root)
        .canonicalize()
        .context(format!(
            "Failed to access ROOT_DIR '{}'. Ensure the path exists.",
            requested_root
        ))
}

fn diag_ok(message: impl std::fmt::Display) {
    println!("\x1b[32mOK\x1b[0m {}", message);
}

fn diag_bad(kind: &str, message: impl std::fmt::Display) {
    println!("\x1b[31m{}\x1b[0m {}", kind, message);
}

async fn run_diagnostics(
    root_str: String,
    port_opt: Option<u16>,
    host_opt: Option<String>,
    config_path: String,
) -> Result<bool> {
    let mut ok = true;
    println!("EasyTex diagnostics");
    println!("Config file: {}", config_path);

    let cfg = match load_global_config(&config_path) {
        Ok(cfg) => {
            diag_ok("config parsed");
            cfg
        }
        Err(e) => {
            diag_bad("FAIL", format!("config: {}", e));
            return Ok(false);
        }
    };

    for tool in ["tectonic", "latexmk", "texfot"] {
        if command_exists(tool) {
            diag_ok(format!("tool: {}", tool));
        } else {
            diag_bad("WARN", format!("tool missing: {}", tool));
        }
    }
    for tool in OPTIONAL_TOOLS {
        if command_exists(tool.command) {
            diag_ok(format!(
                "optional tool: {} ({})",
                tool.command, tool.capability
            ));
        } else {
            diag_bad(
                "WARN",
                format!(
                    "optional tool missing: {} ({})",
                    tool.command, tool.description
                ),
            );
        }
    }

    let requested_root = if root_str == "." {
        cfg.root_dir.as_str()
    } else {
        root_str.as_str()
    };
    let root = match PathBuf::from(requested_root).canonicalize() {
        Ok(root) => {
            diag_ok(format!("root readable: {}", root.display()));
            root
        }
        Err(e) => {
            diag_bad("FAIL", format!("root '{}': {}", requested_root, e));
            ok = false;
            PathBuf::from(requested_root)
        }
    };

    if root.is_dir() {
        let probe = root.join(".easytex-write-test");
        match std::fs::write(&probe, b"ok").and_then(|_| std::fs::remove_file(&probe)) {
            Ok(()) => diag_ok("root writable"),
            Err(e) => {
                diag_bad("FAIL", format!("root writable: {}", e));
                ok = false;
            }
        }
    }

    let host = host_opt.unwrap_or_else(|| cfg.host.clone());
    let port = port_opt.unwrap_or(cfg.port);
    match StdTcpListener::bind(format!("{}:{}", host, port)) {
        Ok(listener) => {
            drop(listener);
            diag_ok(format!("port free: {}:{}", host, port));
        }
        Err(e) => {
            diag_bad("FAIL", format!("port unavailable {}:{}: {}", host, port, e));
            ok = false;
        }
    }

    let history = history_path(&root, &cfg.history_file);
    if let Some(parent) = history.parent() {
        if parent.exists() {
            diag_ok(format!("history directory: {}", parent.display()));
        } else {
            diag_bad(
                "WARN",
                format!("history directory will be created: {}", parent.display()),
            );
        }
    }

    if ok {
        diag_ok("Diagnostics passed");
    } else {
        diag_bad("FAIL", "Diagnostics found blocking issues");
    }
    Ok(ok)
}

async fn run_server(
    root_str: String,
    port_opt: Option<u16>,
    host_opt: Option<String>,
    config_path: String,
) -> Result<()> {
    let g_cfg = load_global_config(&config_path)?;
    check_runtime_dependencies(g_cfg.compress_pdf)?;

    info!(
        "Current working directory: {:?}",
        std::env::current_dir().unwrap_or_default()
    );
    let root = resolve_root(&root_str, &g_cfg)?;
    let history_path = history_path(&root, &g_cfg.history_file);
    let history = load_build_history(&history_path).await;
    let capabilities = RuntimeCapabilities::detect(g_cfg.read_only);
    let frontend_assets = Arc::new(FrontendAssets::load()?);
    info!(
        "Loaded {} embedded frontend assets from compressed archive",
        frontend_assets.len()
    );

    let state = AppState {
        root: root.clone(),
        sessions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        config: Arc::new(g_cfg.clone()),
        build_semaphore: Arc::new(tokio::sync::Semaphore::new(g_cfg.max_concurrent_builds)),
        history: Arc::new(tokio::sync::Mutex::new(history)),
        history_path,
        capabilities,
        frontend_assets,
    };

    let port = port_opt.unwrap_or(g_cfg.port);
    let host = host_opt.unwrap_or_else(|| g_cfg.host.clone());
    g_cfg.validate_effective_bind_host(&host)?;
    let addr = format!("{}:{}", host, port);
    eprintln!("Server is running at http://{}:{}", host, port);

    let state_c = state.clone();
    let ttl_secs = g_cfg.session_ttl_hours * 3600;
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            cleanup_expired_sessions(&state_c, ttl_secs).await;
        }
    });

    let app = Router::new()
        .route("/health", get(|| async { "OK" }))
        .route("/ready", get(|| async { "OK" }))
        .route("/admin", get(admin_dashboard))
        .route("/api/admin/metrics", get(admin_metrics))
        .route("/api/:cmd", any(api_handler_no_name))
        .route("/api/:cmd/:name", any(api_handler))
        .route("/api/admin/:cmd/:name", any(admin_api_handler))
        .route("/events/:name", get(sse_handler))
        .route("/pdf/:name", get(get_pdf))
        .fallback(static_handler)
        .layer(middleware::from_fn(logging_middleware))
        .layer(cors_layer(&g_cfg))
        .with_state(state.clone());

    info!("EasyTex starting");
    info!("Serving projects from: {}", root.display());
    info!("Dashboard available at: http://{}:{}", host, port);
    info!("Admin available at: http://{}:{}/admin", host, port);

    let shutdown_state = state.clone();
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_state))
        .await?;
    Ok(())
}

async fn shutdown_signal(state: AppState) {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            warn!("Failed to install Ctrl+C shutdown handler: {}", e);
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(e) => warn!("Failed to install SIGTERM shutdown handler: {}", e),
        }
    };

    #[cfg(windows)]
    let terminate = async {
        use tokio::signal::windows;

        let ctrl_break = async {
            match windows::ctrl_break() {
                Ok(mut signal) => {
                    signal.recv().await;
                }
                Err(e) => warn!("Failed to install Ctrl+Break shutdown handler: {}", e),
            }
        };
        let ctrl_close = async {
            match windows::ctrl_close() {
                Ok(mut signal) => {
                    signal.recv().await;
                }
                Err(e) => warn!("Failed to install Ctrl+Close shutdown handler: {}", e),
            }
        };
        let ctrl_shutdown = async {
            match windows::ctrl_shutdown() {
                Ok(mut signal) => {
                    signal.recv().await;
                }
                Err(e) => warn!("Failed to install Ctrl+Shutdown handler: {}", e),
            }
        };

        tokio::select! {
            _ = ctrl_break => {},
            _ = ctrl_close => {},
            _ = ctrl_shutdown => {},
        }
    };

    #[cfg(not(any(unix, windows)))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown requested; cancelling active sessions");
    let sessions = {
        let map = state.sessions.lock().await;
        map.values().cloned().collect::<Vec<_>>()
    };
    for sess_arc in sessions {
        let mut sess = sess_arc.lock().await;
        crate::state::cancel_session(&mut sess).await;
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{header, HeaderMap, HeaderValue};

    use crate::config::Config;
    use crate::config::GlobalConfig;
    use crate::utils::{is_valid_entrypoint, is_valid_project_name, safe_path, safe_project_file};

    #[test]
    fn test_is_valid_project_name() {
        let long_name = "a".repeat(257);
        let cases = vec![
            ("valid", true),
            ("valid-name", true),
            ("valid_name", true),
            ("valid123", true),
            ("invalid name", false),
            ("../etc/passwd", false),
            ("etc/passwd", false),
            ("", false),
            (&long_name, false),
            ("project#1", false),
            ("project$2", false),
            ("project!", false),
            (".", false),
            ("..", false),
            ("_start", true),
            ("-start", true),
            ("123", true),
            ("a-b_c", true),
            ("a--b", true),
            ("a__b", true),
            ("a.", false),
            (".a", false),
            (" ", false),
            ("\t", false),
            ("\n", false),
        ];
        for (input, expected) in cases {
            assert_eq!(
                is_valid_project_name(input),
                expected,
                "Failed for: {}",
                input
            );
        }
    }

    #[test]
    fn test_is_valid_entrypoint() {
        let cases = vec![
            ("main.tex", true),
            ("chapter1.tex", true),
            ("main", false),
            ("main.txt", false),
            ("main.tex; rm -rf /", false),
            ("../main.tex", false),
            ("/main.tex", false),
            ("main.tex\0", false),
            ("main.tex.pdf", false),
            ("sub/main.tex", false),
            ("MAIN.TEX", false),
            (".tex", false),
            ("a.tex.tex", true),
            ("a.b.tex", true),
            ("a..tex", false),
            (" main.tex", false),
            ("main.tex ", false),
            ("main.tex\n", false),
            ("main.tex\r", false),
            ("main\t.tex", false),
        ];
        for (input, expected) in cases {
            assert_eq!(
                is_valid_entrypoint(input),
                expected,
                "Failed for: {}",
                input
            );
        }
    }

    #[test]
    fn test_safe_path() {
        let root = std::env::temp_dir().join(format!("easytex-test-root-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        let proj = root.join("demo");
        let _ = std::fs::create_dir_all(&proj);

        let cases = vec![
            ("demo", true),
            ("../passwd", false),
            ("/etc/passwd", false),
            ("invalid-name!", false),
            ("", false),
        ];
        for (name, is_ok) in cases {
            let res = safe_path(&root, name);
            if is_ok {
                assert!(res.is_some(), "Should be safe: {}", name);
            } else {
                assert!(res.is_none(), "Should be unsafe: {}", name);
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_safe_project_file() {
        let root = std::env::temp_dir().join(format!("easytex-test-files-{}", std::process::id()));
        let _ = std::fs::create_dir_all(root.join("chapters"));

        assert!(safe_project_file(&root, "main.tex").is_some());
        assert!(safe_project_file(&root, "chapters/intro.tex").is_some());
        assert!(safe_project_file(&root, "../main.tex").is_none());
        assert!(safe_project_file(&root, "/tmp/main.tex").is_none());
        assert!(safe_project_file(&root, ".env").is_none());
        assert!(safe_project_file(&root, "build/generated.tex").is_none());
        assert!(safe_project_file(&root, "build/output.sh").is_none());
        assert!(safe_project_file(&root, r"chapters\intro.tex").is_none());
        assert!(safe_project_file(&root, "C:/tmp/main.tex").is_none());
        assert!(safe_project_file(&root, "main.tex:evil").is_none());
        assert!(safe_project_file(&root, "//server/share/main.tex").is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_config_default() {
        let cfg = Config::default();
        assert_eq!(cfg.entrypoint, "main.tex");
    }

    #[test]
    fn test_global_config_default() {
        let cfg = GlobalConfig::default();
        assert_eq!(cfg.port, 8081);
        assert_eq!(cfg.host, "127.0.0.1");
        assert_eq!(cfg.max_concurrent_builds, 4);
        assert_eq!(cfg.history_file, ".easytex-history.json");
        assert_eq!(cfg.max_edit_file_size_bytes, 1_000_000);
        assert_eq!(cfg.max_read_file_size_bytes, 2_000_000);
        assert_eq!(cfg.max_project_files, 5_000);
        assert_eq!(cfg.max_pdf_size_bytes, 100 * 1024 * 1024);
        assert!(!cfg.require_auth);
        assert!(!cfg.read_only);
    }

    #[test]
    fn test_auth_config_validation() {
        let mut cfg = GlobalConfig {
            require_auth: true,
            ..GlobalConfig::default()
        };
        assert!(cfg.validate().is_err());

        cfg.admin_token = Some("test-token".into());
        assert!(cfg.validate().is_ok());

        let wildcard = GlobalConfig {
            cors_allowed_origins: vec!["*".into()],
            ..GlobalConfig::default()
        };
        assert!(wildcard.validate().is_err());

        let cfg = GlobalConfig::default();
        assert!(cfg.validate_effective_bind_host("0.0.0.0").is_err());

        let cfg = GlobalConfig {
            admin_token: Some("test-token".into()),
            ..GlobalConfig::default()
        };
        assert!(cfg.validate_effective_bind_host("0.0.0.0").is_ok());
    }

    #[test]
    fn test_redacted_headers_hide_sensitive_values() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer secret"),
        );
        headers.insert(header::COOKIE, HeaderValue::from_static("session=secret"));
        headers.insert("x-api-key", HeaderValue::from_static("secret-key"));
        headers.insert("x-custom-token", HeaderValue::from_static("custom-secret"));
        headers.insert(header::USER_AGENT, HeaderValue::from_static("EasyTexTest"));

        let redacted = super::redacted_headers(&headers);
        assert!(redacted.contains(&("authorization".into(), "[REDACTED]".into())));
        assert!(redacted.contains(&("cookie".into(), "[REDACTED]".into())));
        assert!(redacted.contains(&("x-api-key".into(), "[REDACTED]".into())));
        assert!(redacted.contains(&("x-custom-token".into(), "[REDACTED]".into())));
        assert!(redacted.contains(&("user-agent".into(), "EasyTexTest".into())));
    }
}
