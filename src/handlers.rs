//! # Axum Route Handlers & Dashboard API Module
//!
//! This module forms the API gateway of EasyTex.
//! It exposes all HTTP route handlers and WebSocket/SSE endpoints processed by the Axum server:
//!
//! * **Web GUI Handlers**: Serving the compiled HTML index dashboard, custom CSS/JS assets, and the embedded PDF viewer.
//! * **Telemetry Streams**: Broadcasting Server-Sent Events (SSE) detailing compilation logs and diagnostics to open tabs.
//! * **Project Manager Handlers**: Listing workspace files, creating projects, editing source LaTeX, and updating config mappings.
//! * **Quality & PDF Helpers**: Serving SyncTeX bidirectional coordinate queries, linter reports, formatting triggers, and PDF document delivery.
//! * **Admin Interface Handlers**: Dashboard administration tools and session eviction engines.

use axum::{
    body::Body,
    extract::{FromRequest, Path, Query, Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    response::{
        sse::{Event, Sse},
        Html, IntoResponse, Response,
    },
    Json,
};
use notify::{RecursiveMode, Watcher};
use serde::Deserialize;
use std::{
    path::{Path as FsPath, PathBuf},
    time::Duration,
};
use tokio::process::Command;
use tracing::{debug, info, trace, warn};

use crate::artifacts::{
    get_preview_paths, latest_success_preview, success_preview_by_run, success_previews,
    valid_success_run,
};
use crate::builder::run_build;
use crate::config::{read_cfg, Config};
use crate::dto::{
    BuildArtifactResponse, ConfigResponse, FileListResponse, FileResponse, LintResponse,
    PreviewResponse, ProjectStatusResponse, ProjectsResponse, SynctexEditResponse,
    SynctexViewResponse,
};
use crate::errors::{ok_json, AppError};
use crate::events;
use crate::frontend_assets::FRONTEND_DIST_HASH;
use crate::fs_safety;
use crate::state::{cancel_session, get_or_create_session, AppState, BuildPriority};
use crate::utils::{
    is_valid_entrypoint, is_valid_project_name, safe_path, safe_project_file, MAX_CONFIG_SIZE,
};

/// Extensions list monitored by the notify-watcher loop to trigger incremental builds.
const WATCH_EXTENSIONS: &[&str] = &[
    "tex", "toml", "bib", "sty", "cls", "png", "jpg", "pdf", "tikz", "eps", "svg",
];

/// Input raw TOML wrapper representing saving project configs.
#[derive(Deserialize)]
pub struct ConfigInput {
    /// Raw TOML settings string.
    pub raw: String,
}

/// Query options configuring PDF file retrieval.
#[derive(Deserialize)]
pub struct PdfQuery {
    /// Optional field triggering full document file attachment download mode in browser headers.
    pub dl: Option<String>,
    /// Target specific compile run ID from history, falls back to the latest success if `None`.
    pub run: Option<String>,
}

/// Query parameters selecting a specific target project file relative path.
#[derive(Deserialize)]
struct FileQuery {
    /// Desired sandboxed file path.
    path: Option<String>,
}

/// Payload sent by editor save actions containing the new text payload buffer.
#[derive(Deserialize)]
struct FileInput {
    /// Textual document buffer to save.
    content: String,
    /// Custom relative path target.
    path: Option<String>,
}

/// Coordinate mapping request payload configuring SyncTeX navigation search queries.
#[derive(Deserialize)]
struct SynctexQuery {
    /// Operational mode. Typically `"edit"` (PDF-to-Source) or `"view"` (Source-to-PDF).
    mode: String,
    /// Compilation page number (1-based index) clicked inside the PDF.
    page: Option<u32>,
    /// Horizontal click coordinate (PDF points).
    x: Option<f32>,
    /// Vertical click coordinate (PDF points).
    y: Option<f32>,
    /// Editor cursor source line.
    line: Option<u32>,
    /// Editor cursor source column.
    col: Option<u32>,
    /// Editor target source path.
    path: Option<String>,
}

/// Helper building a standard Axum Response containing a customized content header and body.
fn response_with_body(content_type: &'static str, body: impl Into<Body>) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(body.into())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Authorizes admin-level operations by verifying the Bearer token in headers.
fn admin_authorized(headers: &HeaderMap, state: &AppState) -> bool {
    let Some(token) = state.config.admin_token.as_deref() else {
        return true;
    };
    headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|v| v == token)
}

/// Helper producing standard unauthorized responses.
fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "Admin token required").into_response()
}

/// Helper producing standard read-only error responses.
fn read_only_error() -> Response {
    AppError::ReadOnly.into_response()
}

/// Sends log strings to the active watch channels of a specific project.
async fn session_log(state: &AppState, name: &str, lvl: &str, message: impl Into<String>) {
    let sess = get_or_create_session(state, name).await;
    let _ = sess.lock().await.tx.send(events::log(lvl, message.into()));
}

/// Simple extension check mapping paths to allowed categories.
fn extension_matches(path: &FsPath, allowed: &[&str]) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|s| allowed.contains(&s.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Resolves absolute preview PDF file paths.
async fn preview_pdf_path(
    project_dir: &FsPath,
    entrypoint: &str,
    run: Option<&str>,
) -> Option<PathBuf> {
    match run {
        Some(run) => success_preview_by_run(project_dir, entrypoint, run)
            .await
            .map(|preview| preview.pdf_path),
        None => get_preview_paths(project_dir, entrypoint)
            .await
            .map(|(pdf, _)| pdf),
    }
}

/// Unified Axum asset router serving pre-compiled web GUI files from the in-memory decompressor.
///
/// Matches target paths dynamically and routes file requests to the embedded asset lookup.
/// Defaults to serving the index page on any unknown path to support Single-Page Application (SPA) routing.
pub async fn static_handler(State(st): State<AppState>, req: Request) -> Response {
    let _ = FRONTEND_DIST_HASH;
    let path = req.uri().path().trim_start_matches('/');

    if path.is_empty() {
        return serve_index(&st);
    }

    if let Some(file) = st.frontend_assets.get(path) {
        let mime = match path.split('.').next_back() {
            Some("js") | Some("mjs") => "application/javascript",
            Some("css") => "text/css",
            Some("svg") => "image/svg+xml",
            Some("png") => "image/png",
            Some("html") => "text/html",
            _ => "application/octet-stream",
        };
        return response_with_body(mime, Body::from(file.to_vec()));
    }

    serve_index(&st)
}

/// Helper serving the decompressed byte buffer of `index.html` as a `"text/html"` response.
fn serve_index(st: &AppState) -> Response {
    let Some(file) = st.frontend_assets.get("index.html") else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    response_with_body("text/html", Body::from(file.to_vec()))
}

pub async fn get_pdf(
    Path(name): Path<String>,
    Query(q): Query<PdfQuery>,
    State(st): State<AppState>,
) -> Response {
    let Some(proj) = safe_path(&st.root, &name) else {
        warn!("PDF request denied - invalid project path: {}", name);
        return StatusCode::BAD_REQUEST.into_response();
    };

    debug!("PDF request for project: {}", name);

    let (cfg, _) = read_cfg(&st.root, &name).await;
    let Some(path) = preview_pdf_path(&proj, &cfg.entrypoint, q.run.as_deref()).await else {
        let map = st.sessions.lock().await;
        if let Some(sess_arc) = map.get(&name) {
            let sess = sess_arc.lock().await;
            if sess.process.is_some() {
                debug!("PDF request deferred - build in progress for: {}", name);
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
        }

        warn!("PDF not found for project: {}", name);
        return StatusCode::NOT_FOUND.into_response();
    };

    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => {
            info!("Serving PDF for {}: {:?}", name, path.file_name());
            f
        }
        Err(e) => {
            warn!("Failed to open PDF for {}: {}", name, e);
            return StatusCode::NOT_FOUND.into_response();
        }
    };
    if let Ok(metadata) = file.metadata().await {
        if metadata.len() > st.config.max_pdf_size_bytes {
            return AppError::PayloadTooLarge(format!(
                "PDF too large ({} bytes, max {} bytes)",
                metadata.len(),
                st.config.max_pdf_size_bytes
            ))
            .into_response();
        }
    }

    let stream = tokio_util::io::ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let mut res = body.into_response();
    let headers = res.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/pdf"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache, no-store, must-revalidate"),
    );

    if q.dl.as_deref() == Some("1") {
        let suffix = q
            .run
            .as_deref()
            .filter(|run| valid_success_run(run))
            .map(|run| format!("-{}", run))
            .unwrap_or_default();
        let cd = format!("attachment; filename=\"{}{}.pdf\"", name, suffix);
        if let Ok(val) = header::HeaderValue::from_str(&cd) {
            headers.insert(header::CONTENT_DISPOSITION, val);
            info!("Downloading PDF for {}", name);
        }
    }

    res
}

pub async fn sse_handler(Path(name): Path<String>, State(st): State<AppState>) -> Response {
    if safe_path(&st.root, &name).is_none() {
        warn!("SSE connection rejected - invalid project: {}", name);
        return StatusCode::BAD_REQUEST.into_response();
    }

    info!("SSE connection established for project: {}", name);

    let sess_arc = get_or_create_session(&st, &name).await;
    let mut sess = sess_arc.lock().await;

    if sess._watcher.is_none() {
        debug!("Setting up file watcher for {}", name);

        let (tx_ev, mut rx_ev) = tokio::sync::mpsc::channel(100);
        let proj_dir = st
            .root
            .join(&name)
            .canonicalize()
            .unwrap_or_else(|_| st.root.join(&name));
        let st_c = st.clone();
        let name_c = name.clone();
        let name_watcher = name.clone();
        let proj_dir_c = proj_dir.clone();
        let tx_ev_c = tx_ev.clone();
        let sess_tx_c = sess.tx.clone();

        let watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
                Ok(ev) => {
                    for path in ev.paths {
                        if path.components().any(|c| c.as_os_str() == "build") {
                            continue;
                        }
                        if extension_matches(&path, WATCH_EXTENSIONS) {
                            trace!("File modified: {:?}", path);
                            info!(
                                "Watcher [{}]: Modification detected in {:?}",
                                name_watcher,
                                path.file_name().unwrap_or_default()
                            );

                            if let Ok(abs_path) = path.canonicalize() {
                                if let Ok(rel_path) = abs_path.strip_prefix(&proj_dir_c) {
                                    let rel_str = rel_path.to_string_lossy().to_string();
                                    debug!("Broadcasting file_changed for {}", rel_str);
                                    let _ = sess_tx_c.send(events::file_changed(rel_str));
                                }
                            }

                            let _ = tx_ev_c.try_send(());
                            break;
                        }
                    }
                }
                Err(e) => warn!("File watcher error for {}: {}", name_watcher, e),
            });
        let Ok(mut w) = watcher else {
            warn!("Failed to create file watcher for {}", name);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        if let Err(e) = w.watch(&proj_dir, RecursiveMode::Recursive) {
            warn!("Failed to watch {}: {}", proj_dir.display(), e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        sess._watcher = Some(Box::new(w));

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            info!("Initial build triggered for {}", name_c);
            let _ = run_build(
                st_c.clone(),
                name_c.clone(),
                BuildPriority::Auto,
                "Initial Build",
            )
            .await;

            while rx_ev.recv().await.is_some() {
                tokio::time::sleep(Duration::from_millis(300)).await;
                while rx_ev.try_recv().is_ok() {}
                info!("Auto-build triggered for {}", name_c);
                run_build(
                    st_c.clone(),
                    name_c.clone(),
                    BuildPriority::Auto,
                    "Auto-Build",
                )
                .await;
            }
        });
    }

    let tx = sess.tx.clone();
    drop(sess);

    let mut rx = tx.subscribe();
    let stream = async_stream::stream! {
        debug!("SSE stream started for {}", name);
        loop {
            match rx.recv().await {
                Ok(msg) => yield Ok::<Event, std::convert::Infallible>(Event::default().data(msg)),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    warn!("SSE message lag detected for {}", name);
                    continue;
                },
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    info!("SSE stream closed for {}", name);
                    break;
                },
            }
        }
    };

    Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::new().interval(Duration::from_secs(10)))
        .into_response()
}

pub async fn admin_dashboard(headers: HeaderMap, State(state): State<AppState>) -> Response {
    if !admin_authorized(&headers, &state) {
        return unauthorized();
    }

    let map = state.sessions.lock().await;
    let mut session_info = Vec::new();
    for (name, sess_arc) in map.iter() {
        if let Ok(sess) = sess_arc.try_lock() {
            session_info.push(serde_json::json!({
                "name": name,
                "pid": sess.process.as_ref().map(|process| process.pid()),
                "age": std::time::Instant::now().duration_since(sess.last_accessed).as_secs(),
            }));
        }
    }

    let history = state.history.lock().await;

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8"><title>EasyTex Admin</title>
    <style>
        body {{ font-family: system-ui; background: #0f172a; color: #f8fafc; padding: 2rem; max-width: 1200px; margin: 0 auto; }}
        table {{ width: 100%; border-collapse: collapse; margin-top: 1rem; background: #1e293b; border-radius: 8px; overflow: hidden; }}
        th, td {{ padding: 1rem; text-align: left; border-bottom: 1px solid #334155; }}
        th {{ background: #334155; color: #94a3b8; font-weight: 600; text-transform: uppercase; font-size: 0.75rem; letter-spacing: 0.05em; }}
        tr:last-child td {{ border-bottom: none; }}
        h1 {{ color: #38bdf8; margin-bottom: 2rem; }}
        h2 {{ color: #f1f5f9; margin-top: 3rem; }}
        .btn {{ padding: 0.5rem 1rem; cursor: pointer; background: #ef4444; border: none; color: white; border-radius: 4px; font-weight: 600; }}
        .btn:hover {{ background: #dc2626; }}
        .stats {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 1.5rem; margin-bottom: 3rem; }}
        .stat-card {{ background: #1e293b; padding: 1.5rem; border-radius: 8px; border: 1px solid #334155; }}
        .stat-val {{ font-size: 2rem; font-weight: 700; color: #38bdf8; }}
        .stat-label {{ color: #94a3b8; font-size: 0.875rem; }}
    </style>
</head>
<body>
    <h1>EasyTex Admin</h1>
    <div class="stats">
        <div class="stat-card"><div class="stat-val">{count}</div><div class="stat-label">Active Sessions</div></div>
        <div class="stat-card"><div class="stat-val">{hist_count}</div><div class="stat-label">Total Builds</div></div>
    </div>
    <h2>Active Sessions</h2>
    <table>
        <thead><tr><th>Project</th><th>PID</th><th>Idle Time</th><th>Action</th></tr></thead>
        <tbody>{sessions}</tbody>
    </table>
    <h2>Recent Build History</h2>
    <table>
        <thead><tr><th>Project</th><th>Label</th><th>Time</th><th>Duration</th><th>Status</th></tr></thead>
        <tbody>{history}</tbody>
    </table>
</body>
</html>"#,
        count = map.len(),
        hist_count = history.len(),
        sessions = session_info.iter().map(|s| {
            format!(
                "<tr><td>{}</td><td>{:?}</td><td>{}s</td><td><button class='btn' onclick=\"fetch('/api/admin/kill/{}', {{method:'POST'}}).then(()=>location.reload())\">Kill</button></td></tr>",
                s["name"], s["pid"], s["age"], s["name"]
            )
        }).collect::<Vec<_>>().join(""),
        history = history.iter().rev().map(|h| {
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{:.1}s</td><td>{}</td></tr>",
                h.project, h.label, h.timestamp, h.duration, h.status
            )
        }).collect::<Vec<_>>().join("")
    );

    Html(html).into_response()
}

pub async fn admin_api_handler(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path((cmd, name)): Path<(String, String)>,
) -> Response {
    if !admin_authorized(&headers, &state) {
        return unauthorized();
    }

    if !is_valid_project_name(&name) {
        warn!("Admin API - invalid project name: {}", name);
        return StatusCode::BAD_REQUEST.into_response();
    }

    debug!("Admin API command '{}' on project '{}'", cmd, name);

    match cmd.as_str() {
        "kill" => {
            let mut map = state.sessions.lock().await;
            if let Some(sess_arc) = map.remove(&name) {
                info!("Killing build session for {}", name);
                let mut sess = sess_arc.lock().await;
                cancel_session(&mut sess).await;
                info!("Build process terminated for {}", name);
            } else {
                debug!("No active session to kill for {}", name);
            }
            StatusCode::OK.into_response()
        }
        _ => {
            warn!("Unknown admin command: {}", cmd);
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

pub async fn api_handler_no_name(
    Path(cmd): Path<String>,
    state: State<AppState>,
    req: Request,
) -> Response {
    api_handler(Path((cmd, String::new())), state, req).await
}

pub async fn api_handler(
    Path((cmd, name)): Path<(String, String)>,
    State(st): State<AppState>,
    req: Request,
) -> Response {
    let method = req.method().clone();
    
    // CSRF protection for all mutative POST API requests
    if method == Method::POST {
        let has_csrf_header = req.headers()
            .get("X-EasyTex-Request")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|v| v == "true");
        if !has_csrf_header {
            return (
                StatusCode::FORBIDDEN,
                "CSRF protection: X-EasyTex-Request header is missing or invalid",
            )
                .into_response();
        }
    }

    let uri = req.uri().clone();
    if cmd == "projects" {
        return list_projects(&st).await;
    }
    if cmd == "capabilities" {
        return capabilities(&st).await;
    }

    let Some(project_dir) = safe_path(&st.root, &name) else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    debug!("API command '{}' for project '{}'", cmd, name);

    match cmd.as_str() {
        "build" | "run" => {
            if st.config.read_only {
                return read_only_error();
            }
            info!("Build requested for {}", name);
            run_build(st, name, BuildPriority::Manual, "Manual Build").await;
            return StatusCode::OK.into_response();
        }
        "cancel" => {
            if st.config.read_only {
                return read_only_error();
            }
            let map = st.sessions.lock().await;
            if let Some(arc) = map.get(&name) {
                let mut sess = arc.lock().await;
                cancel_session(&mut sess).await;
                let _ = sess.tx.send(events::status("Idle"));
                let _ = sess.tx.send(events::log("err", "Build cancelled by user."));
            }
        }
        "status" => return project_status(&st, &name).await,
        "format" => return format_project(&st, &name, &project_dir).await,
        "lint" => return lint_project(&st, &name, &project_dir).await,
        "clean" => {
            if st.config.read_only {
                return read_only_error();
            }
            if let Err(e) = tokio::fs::remove_dir_all(project_dir.join("build")).await {
                debug!("Clean skipped or failed for {}: {}", name, e);
            }
            session_log(&st, &name, "ok", "Cleaned build directory.").await;
        }
        "delete" => {
            if st.config.read_only {
                return read_only_error();
            }
            if let Err(e) = tokio::fs::remove_file(project_dir.join("EasyTex.toml")).await {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to delete config: {}", e),
                )
                    .into_response();
            }
            st.sessions.lock().await.remove(&name);
            return StatusCode::OK.into_response();
        }
        "create" => {
            if st.config.read_only {
                return read_only_error();
            }
            return create_project(&st, &name).await;
        }
        "config" if method == Method::GET => {
            let (_, raw) = read_cfg(&st.root, &name).await;
            return Json(ConfigResponse { raw }).into_response();
        }
        "config" => {
            if st.config.read_only {
                return read_only_error();
            }
            return save_config(&project_dir, req).await;
        }
        "preview" => return preview_info(&st, &name, &project_dir).await,
        "builds" => return list_success_builds(&st, &name, &project_dir).await,
        "file" if method == Method::GET => {
            return read_project_file(&st, &name, &project_dir, &uri).await
        }
        "file" => {
            if st.config.read_only {
                return read_only_error();
            }
            return save_project_file(&st, &name, &project_dir, req).await;
        }
        "files" => return list_project_files(&st, project_dir).await,
        "synctex" => return synctex(&st, &name, &project_dir, &uri).await,
        "js" => return StatusCode::GONE.into_response(),
        _ => return StatusCode::NOT_FOUND.into_response(),
    }
    ok_json()
}

async fn list_projects(st: &AppState) -> Response {
    let mut items = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&st.root).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            if e.path().is_dir() && e.path().join("EasyTex.toml").exists() {
                if let Some(n) = e.file_name().to_str() {
                    items.push(n.to_string());
                }
            }
        }
    }
    items.sort();
    info!("Listed {} projects", items.len());
    Json(ProjectsResponse { projects: items }).into_response()
}

async fn capabilities(st: &AppState) -> Response {
    Json(&st.capabilities).into_response()
}

async fn project_status(st: &AppState, name: &str) -> Response {
    let map = st.sessions.lock().await;
    let status = if let Some(sess_arc) = map.get(name) {
        if let Ok(sess) = sess_arc.try_lock() {
            if sess.process.is_some() {
                "building"
            } else {
                "idle"
            }
        } else {
            "locked"
        }
    } else {
        "idle"
    };
    Json(ProjectStatusResponse {
        status: status.to_string(),
    })
    .into_response()
}

async fn preview_info(st: &AppState, name: &str, project_dir: &FsPath) -> Response {
    let (cfg, _) = read_cfg(&st.root, name).await;
    let Some(preview) = latest_success_preview(project_dir, &cfg.entrypoint).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let size = preview
        .pdf_path
        .metadata()
        .ok()
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    Json(PreviewResponse {
        run: preview.run,
        built_at_ms: preview.built_at_ms,
        pdf_size_bytes: size,
    })
    .into_response()
}

async fn list_success_builds(st: &AppState, name: &str, project_dir: &FsPath) -> Response {
    let (cfg, _) = read_cfg(&st.root, name).await;
    let builds = success_previews(project_dir, &cfg.entrypoint)
        .await
        .into_iter()
        .map(|preview| {
            let size = preview
                .pdf_path
                .metadata()
                .ok()
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            BuildArtifactResponse {
                run: preview.run,
                built_at_ms: preview.built_at_ms,
                pdf_size_bytes: size,
            }
        })
        .collect::<Vec<_>>();

    Json(builds).into_response()
}

async fn create_project(st: &AppState, name: &str) -> Response {
    let p = st.root.join(name);
    if p.exists() {
        return AppError::Conflict("Project already exists".into()).into_response();
    }
    if let Err(e) = tokio::fs::create_dir_all(&p).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to create project: {}", e),
        )
            .into_response();
    }
    let files = [
        ("EasyTex.toml", "entrypoint = \"main.tex\"\n"),
        (
            "main.tex",
            "\\documentclass{article}\n\\begin{document}\nHello, LaTeX!\n\\end{document}",
        ),
    ];
    for (file, content) in files {
        if let Err(e) = tokio::fs::write(p.join(file), content).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to write {}: {}", file, e),
            )
                .into_response();
        }
    }
    ok_json()
}

async fn save_config(project_dir: &FsPath, req: Request) -> Response {
    let Ok(Json(input)) = Json::<ConfigInput>::from_request(req, &()).await else {
        return AppError::BadRequest("Invalid config request body".into()).into_response();
    };

    if input.raw.len() > MAX_CONFIG_SIZE {
        return AppError::PayloadTooLarge(format!(
            "Config file too large (max {} bytes)",
            MAX_CONFIG_SIZE
        ))
        .into_response();
    }

    match toml::from_str::<Config>(&input.raw) {
        Ok(cfg) => {
            if !is_valid_entrypoint(&cfg.entrypoint) {
                return AppError::BadRequest("Invalid entrypoint: must be a .tex file".into()).into_response();
            }
            if let Some(cmd) = &cfg.format_command {
                let parts: Vec<&str> = cmd.split_whitespace().collect();
                if !parts.is_empty() && parts[0] != "tex-fmt" {
                    return AppError::BadRequest(
                        "Only the 'tex-fmt' formatting utility is permitted for security reasons"
                            .into(),
                    )
                    .into_response();
                }
            }
            match tokio::fs::write(project_dir.join("EasyTex.toml"), &input.raw).await {
                Ok(()) => ok_json(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to save config: {}", e),
                )
                    .into_response(),
            }
        }
        Err(_) => AppError::BadRequest("Invalid TOML syntax".into()).into_response(),
    }
}

async fn project_file_path(
    st: &AppState,
    name: &str,
    project_dir: &FsPath,
    path: Option<String>,
) -> Result<fs_safety::ProjectFile, AppError> {
    let (cfg, _) = read_cfg(&st.root, name).await;
    let relative_path = path.unwrap_or(cfg.entrypoint);
    fs_safety::resolve_project_file(project_dir, relative_path)
}

async fn read_project_file(
    st: &AppState,
    name: &str,
    project_dir: &FsPath,
    uri: &axum::http::Uri,
) -> Response {
    let query = match axum::extract::Query::<FileQuery>::try_from_uri(uri) {
        Ok(axum::extract::Query(q)) => q,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let file = match project_file_path(st, name, project_dir, query.path).await {
        Ok(result) => result,
        Err(error) => return error.into_response(),
    };
    match fs_safety::read_text_limited(&file.absolute_path, st.config.max_read_file_size_bytes)
        .await
    {
        Ok(content) => Json(FileResponse {
            content,
            path: file.relative_path,
        })
        .into_response(),
        Err(error) => error.into_response(),
    }
}

async fn save_project_file(
    st: &AppState,
    name: &str,
    project_dir: &FsPath,
    req: Request,
) -> Response {
    let Ok(Json(input)) = Json::<FileInput>::from_request(req, st).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let file = match project_file_path(st, name, project_dir, input.path).await {
        Ok(result) => result,
        Err(error) => return error.into_response(),
    };
    match fs_safety::write_text_limited(
        &file.absolute_path,
        &input.content,
        st.config.max_edit_file_size_bytes,
    )
    .await
    {
        Ok(()) => ok_json(),
        Err(error) => error.into_response(),
    }
}

async fn list_project_files(st: &AppState, project_dir: PathBuf) -> Response {
    match fs_safety::list_project_files(project_dir, st.config.max_project_files).await {
        Ok(files) => Json(FileListResponse {
            files: files.files,
            complete: files.complete,
        })
        .into_response(),
        Err(error) => error.into_response(),
    }
}

async fn synctex(
    st: &AppState,
    name: &str,
    project_dir: &FsPath,
    uri: &axum::http::Uri,
) -> Response {
    if !st.capabilities.synctex {
        return (
            StatusCode::NOT_FOUND,
            "SyncTeX is unavailable: synctex is not in PATH",
        )
            .into_response();
    }

    let query = match axum::extract::Query::<SynctexQuery>::try_from_uri(uri) {
        Ok(axum::extract::Query(q)) => q,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    let (cfg, _) = read_cfg(&st.root, name).await;
    let Some((pdf_path, synctex_path)) = get_preview_paths(project_dir, &cfg.entrypoint).await
    else {
        return (StatusCode::NOT_FOUND, "PDF not found for SyncTeX").into_response();
    };
    if !synctex_path.exists() {
        return (
            StatusCode::NOT_FOUND,
            "SyncTeX data not found; rebuild the project",
        )
            .into_response();
    }

    match query.mode.as_str() {
        "edit" => synctex_edit(project_dir, &pdf_path, &query).await,
        "view" => {
            let entry = query.path.clone().unwrap_or_else(|| cfg.entrypoint.clone());
            if safe_project_file(project_dir, &entry).is_none() {
                return StatusCode::FORBIDDEN.into_response();
            }
            synctex_view(project_dir, &pdf_path, &cfg.entrypoint, query).await
        }
        _ => StatusCode::BAD_REQUEST.into_response(),
    }
}

async fn synctex_edit(project_dir: &FsPath, pdf_path: &FsPath, query: &SynctexQuery) -> Response {
    let output = Command::new("synctex")
        .args([
            "edit",
            "-o",
            &format!(
                "{}:{}:{}:{}",
                query.page.unwrap_or(1),
                query.x.unwrap_or(0.0),
                query.y.unwrap_or(0.0),
                pdf_path.display()
            ),
        ])
        .current_dir(project_dir)
        .output()
        .await;

    let Ok(output) = output else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return (StatusCode::BAD_REQUEST, error.trim().to_string()).into_response();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let file = synctex_value(&stdout, "Input:").unwrap_or_default();
    let line = synctex_num(&stdout, "Line:", 1);
    let column = synctex_num(&stdout, "Column:", 1);

    if file.is_empty() {
        return (StatusCode::NOT_FOUND, "SyncTeX source location not found").into_response();
    }

    let Some(file) = project_dir.join(&file).canonicalize().ok().and_then(|p| {
        p.strip_prefix(project_dir)
            .ok()
            .map(|p| p.to_string_lossy().to_string())
    }) else {
        return StatusCode::FORBIDDEN.into_response();
    };

    Json(SynctexEditResponse { file, line, column }).into_response()
}

async fn synctex_view(
    project_dir: &FsPath,
    pdf_path: &FsPath,
    entrypoint: &str,
    query: SynctexQuery,
) -> Response {
    let entry = query.path.unwrap_or_else(|| entrypoint.to_string());
    let output = Command::new("synctex")
        .args([
            "view",
            "-i",
            &format!(
                "{}:{}:{}",
                query.line.unwrap_or(1),
                query.col.unwrap_or(1),
                entry
            ),
            "-o",
            &pdf_path.to_string_lossy(),
        ])
        .current_dir(project_dir)
        .output()
        .await;

    let Ok(output) = output else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return (StatusCode::BAD_REQUEST, error.trim().to_string()).into_response();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let page = synctex_num(&stdout, "Page:", 1);
    let x = synctex_num(&stdout, "x:", 0.0);
    let y = synctex_num(&stdout, "y:", 0.0);

    Json(SynctexViewResponse { page, x, y }).into_response()
}

fn synctex_value(output: &str, prefix: &str) -> Option<String> {
    output.lines().find_map(|line| {
        line.strip_prefix(prefix)
            .map(|value| value.trim().to_string())
    })
}

fn synctex_num<T: std::str::FromStr + Copy>(output: &str, prefix: &str, default: T) -> T {
    synctex_value(output, prefix)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

async fn format_project(st: &AppState, name: &str, proj_dir: &FsPath) -> Response {
    if st.config.read_only {
        return read_only_error();
    }

    if !st.capabilities.format {
        return AppError::DependencyMissing("tex-fmt").into_response();
    }

    let (cfg, _) = read_cfg(&st.root, name).await;
    let cmd_str = cfg.format_command.unwrap_or_else(|| "tex-fmt".into());
    let parts: Vec<&str> = cmd_str.split_whitespace().collect();
    if parts.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let program = parts[0];
    if program != "tex-fmt" {
        return (
            StatusCode::BAD_REQUEST,
            "Only the 'tex-fmt' formatting utility is permitted for security reasons.",
        )
            .into_response();
    }

    let mut args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
    if !args.iter_mut().any(|a| {
        let found = a.contains("{file}");
        if found {
            *a = a.replace("{file}", &cfg.entrypoint);
        }
        found
    }) {
        args.push(cfg.entrypoint);
    }

    let output = Command::new(program)
        .args(&args)
        .current_dir(proj_dir)
        .output()
        .await;
    match output {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let message = if stdout.trim().is_empty() {
                format!("Formatted successfully with '{}'.", program)
            } else {
                format!("Formatted: {}", stdout.trim())
            };
            session_log(st, name, "ok", message).await;
            StatusCode::OK.into_response()
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let message = format!("Format command '{}' failed: {}", program, stderr.trim());
            session_log(st, name, "err", message.clone()).await;
            (StatusCode::BAD_REQUEST, message).into_response()
        }
        Err(e) => {
            let message = format!("Failed to execute format command '{}': {}", program, e);
            session_log(st, name, "err", message.clone()).await;
            (StatusCode::BAD_REQUEST, message).into_response()
        }
    }
}

async fn lint_project(st: &AppState, name: &str, proj_dir: &FsPath) -> Response {
    if !st.capabilities.lint {
        return AppError::DependencyMissing("chktex").into_response();
    }

    let (cfg, _) = read_cfg(&st.root, name).await;
    if !is_valid_entrypoint(&cfg.entrypoint) {
        return (
            StatusCode::BAD_REQUEST,
            "Invalid entrypoint: must be a .tex file",
        )
            .into_response();
    }

    match Command::new("chktex")
        .arg(&cfg.entrypoint)
        .current_dir(proj_dir)
        .output()
        .await
    {
        Ok(output) => Json(LintResponse {
            ok: output.status.success(),
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to execute chktex: {}", e),
        )
            .into_response(),
    }
}
