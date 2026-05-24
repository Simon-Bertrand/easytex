use std::{
    path::Path as FsPath,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncBufReadExt,
    process::Command,
    sync::{broadcast, Mutex},
};
use tracing::{debug, info, warn};

use crate::config::read_cfg;
use crate::events;
use crate::fs_safety;
use crate::process;
use crate::state::{
    cancel_session, get_or_create_session, record_build_history, AppState, BuildHistoryEntry,
    BuildPriority, BuildStatus, Session,
};
use crate::utils::{
    is_valid_entrypoint, is_valid_project_name, rand_hex_string, safe_project_file,
};

/// Maximum number of historic run directories kept under `build/runs/` before garbage collection triggers.
const MAX_RUN_HISTORY_DIRS: usize = 10;
/// Minimum threshold of successful run directories guaranteed to be kept from deletion.
const MIN_SUCCESS_RUN_DIRS: usize = 3;
const MAX_WORD_COUNT_FILE_BYTES: usize = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandOutcome {
    Success,
    Failed,
    TimedOut,
}

impl CommandOutcome {
    fn build_status(self) -> BuildStatus {
        match self {
            Self::Success => BuildStatus::Success,
            Self::Failed => BuildStatus::Failed,
            Self::TimedOut => BuildStatus::TimedOut,
        }
    }
}

/// Recursively counts words across the main document and all sub-files loaded via `\input{}` or `\include{}` directives.
///
/// Implements a simple text stack traversal to trace all linked LaTeX dependencies, ignoring duplicate loops.
///
/// # Arguments
///
/// * `proj_dir` - Reference path pointing to the project directory root.
/// * `entrypoint` - Entry LaTeX document filename.
pub async fn get_word_count(proj_dir: &FsPath, entrypoint: &str) -> u32 {
    let mut visited = Vec::new();
    let mut stack = vec![proj_dir.join(entrypoint)];
    let mut total = 0;
    let mut scanned_files = 0usize;

    while let Some(path) = stack.pop() {
        if scanned_files >= 500 {
            break;
        }
        if visited.contains(&path) {
            continue;
        }
        visited.push(path.clone());
        scanned_files += 1;

        if let Ok(content) = fs_safety::read_text_limited(&path, MAX_WORD_COUNT_FILE_BYTES).await {
            total += content.split_whitespace().count() as u32;

            let mut search_pos = 0;
            while let Some((pos, marker_len)) = find_latex_include(&content[search_pos..]) {
                let start = search_pos + pos;
                if let Some(end) = content[start..].find('}') {
                    let mut sub = content[start + marker_len..start + end].trim().to_string();
                    if !sub.ends_with(".tex") {
                        sub.push_str(".tex");
                    }
                    if let Some(sub_path) = safe_project_file(proj_dir, &sub) {
                        stack.push(sub_path);
                    }
                    search_pos = start + end + 1;
                } else {
                    break;
                }
            }
        }
    }
    total
}

fn find_latex_include(haystack: &str) -> Option<(usize, usize)> {
    let input = haystack.find("\\input{").map(|pos| (pos, "\\input{".len()));
    let include = haystack
        .find("\\include{")
        .map(|pos| (pos, "\\include{".len()));
    match (input, include) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Resolves the size of the compiled PDF and returns a human-readable display string (e.g. `"45 KB"`, `"2.4 MB"`).
pub async fn get_pdf_size(pdf_path: &FsPath) -> String {
    if let Ok(meta) = tokio::fs::metadata(&pdf_path).await {
        let kb = meta.len() as f64 / 1024.0;
        if kb > 1024.0 {
            return format!("{:.1} MB", kb / 1024.0);
        }
        return format!("{:.0} KB", kb);
    }
    "0 KB".into()
}

/// Inspects a single standard output line from the compiler and returns its log category classification.
fn log_level_for_stdout(line: &str) -> &'static str {
    if line.starts_with("! ") || line.contains("Error:") || line.contains("not found") {
        "err"
    } else if line.contains("Warning:") || line.contains("corrupt") || line.contains("retry") {
        "warn"
    } else if line.contains("Output written") || line.contains("Latexmk: All targets") {
        "ok"
    } else {
        "dim"
    }
}

/// Inspects a single standard error line from the linter or compiler and returns its log category classification.
fn log_level_for_stderr(line: &str) -> &'static str {
    if line.contains("ERROR") || line.contains("Error:") {
        "err"
    } else if line.contains("WARN") || line.contains("Warning:") {
        "warn"
    } else {
        "dim"
    }
}

/// Parses raw LaTeX error and warning log lines into structured `DiagnosticEvent` objects.
///
/// Matches target filename, line, and column numbers if standard compiler syntax rules are followed.
fn diagnostic_from_log_line(line: &str, level: &str) -> Option<events::DiagnosticEvent> {
    if level != "err" && level != "warn" {
        return None;
    }

    let mut file = None;
    let mut line_no = None;
    let mut column = None;
    let mut message = line.trim().to_string();

    let parts = line.splitn(4, ':').collect::<Vec<_>>();
    if parts.len() >= 3 && parts[0].ends_with(".tex") && parts[1].parse::<u32>().is_ok() {
        file = Some(parts[0].to_string());
        line_no = Some(parts[1].parse::<u32>().unwrap_or(1));
        if parts.len() == 4 && parts[2].parse::<u32>().is_ok() {
            column = Some(parts[2].parse::<u32>().unwrap_or(1));
            message = parts[3].trim().to_string();
        } else {
            message = parts[2..].join(":").trim().to_string();
        }
    }

    Some(events::DiagnosticEvent {
        severity: level.to_string(),
        file,
        line: line_no,
        column,
        message,
        raw: line.to_string(),
    })
}

/// Helper encoding routine broadcasting logs and diagnostics over the session stream channel.
fn emit_log_line(tx: &broadcast::Sender<String>, level: &str, line: String) {
    let _ = tx.send(events::log(level, line.clone()));
    if let Some(diagnostic) = diagnostic_from_log_line(&line, level) {
        let _ = tx.send(events::diagnostic(diagnostic));
    }
}

/// Maps compile outcomes to database entry structs.
fn history_entry(
    project: &str,
    label: &str,
    duration: f32,
    status: BuildStatus,
) -> BuildHistoryEntry {
    BuildHistoryEntry {
        project: project.to_string(),
        timestamp: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        duration,
        status: status.as_str().to_string(),
        label: label.to_string(),
    }
}

/// Scoped RAII guard tracking active compilation tasks.
///
/// Automatically resets execution status tags inside the session lock upon completion or thread panic.
pub struct BuildGuard {
    sess_arc: Arc<Mutex<Session>>,
    generation: u64,
}

impl Drop for BuildGuard {
    fn drop(&mut self) {
        let sess_arc = self.sess_arc.clone();
        let generation = self.generation;
        tokio::spawn(async move {
            let mut s = sess_arc.lock().await;
            if s.build_generation == generation {
                s.process = None;
                s.current_priority = None;
            }
        });
    }
}

/// Garbage-collects old compile run folders under `build/runs/` to free disk space.
///
/// Guarantees that at least `MIN_SUCCESS_RUN_DIRS` successful compile outcomes are locked
/// against automatic removal.
///
/// # Arguments
///
/// * `proj_dir` - Sandbox path pointing to the project directory root.
/// * `active_run_id` - Optional run ID that must be exempted from immediate deletion.
pub async fn clean_old_runs(proj_dir: &FsPath, active_run_id: Option<&str>) {
    let runs_dir = proj_dir.join("build").join("runs");
    let mut entries = match tokio::fs::read_dir(&runs_dir).await {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut runs = Vec::new();

    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(ft) = entry.file_type().await {
            if ft.is_dir() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy().to_string();
                if name_str.ends_with("_S") || name_str.ends_with("_F") {
                    runs.push((name_str, entry.path()));
                }
            }
        }
    }

    if runs.len() <= MAX_RUN_HISTORY_DIRS {
        return;
    }

    runs.sort_by(|a, b| a.0.cmp(&b.0));
    let success_total = runs.iter().filter(|(name, _)| name.ends_with("_S")).count();
    let mut success_remaining = success_total;

    for (name, path) in runs {
        if Some(name.as_str()) == active_run_id {
            continue;
        }
        if runs_dir_count(&runs_dir).await <= MAX_RUN_HISTORY_DIRS {
            break;
        }
        if name.ends_with("_S") && success_remaining <= MIN_SUCCESS_RUN_DIRS {
            continue;
        }
        if name.ends_with("_S") {
            success_remaining = success_remaining.saturating_sub(1);
        }
        debug!("Cleaning old run directory: {}", path.display());
        let _ = tokio::fs::remove_dir_all(&path).await;
    }
}

/// Helper counting the total number of processed compile runs folders.
async fn runs_dir_count(runs_dir: &FsPath) -> usize {
    let mut count = 0;
    let Ok(mut entries) = tokio::fs::read_dir(runs_dir).await else {
        return 0;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_type().await.is_ok_and(|ft| ft.is_dir()) {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with("_S") || name.ends_with("_F") {
                count += 1;
            }
        }
    }
    count
}

/// Scans run folders and locates the absolute latest successful preview directory.
async fn latest_success_run_dir(runs_dir: &FsPath) -> Option<(String, std::path::PathBuf)> {
    let mut entries = tokio::fs::read_dir(runs_dir).await.ok()?;
    let mut latest: Option<(String, std::path::PathBuf)> = None;
    while let Ok(Some(entry)) = entries.next_entry().await {
        if !entry.file_type().await.ok()?.is_dir() {
            continue;
        }
        let run = entry.file_name().to_string_lossy().to_string();
        if !run.ends_with("_S") {
            continue;
        }
        if latest.as_ref().is_none_or(|(current, _)| run > *current) {
            latest = Some((run, entry.path()));
        }
    }
    latest
}

/// Resolves standard timestamp string keys.
fn run_timestamp() -> String {
    chrono::Utc::now().format("%Y%m%d-%H%M%S-%f").to_string()
}

/// Finalizes a temporary compile run directory.
///
/// Renames the completed pending compile output from a `_P` folder into its final outcome folder
/// ending with `_S` (Success) or `_F` (Failed). Handles rename collision by introducing hex strings.
async fn finalize_run_dir(
    run_dir: &FsPath,
    runs_base_dir: &FsPath,
    timestamp: &str,
    suffix: char,
) -> (String, std::path::PathBuf) {
    let mut run_id = format!("{}_{}", timestamp, suffix);
    let mut final_dir = runs_base_dir.join(&run_id);
    if final_dir.exists() {
        run_id = format!("{}-{}_{}", timestamp, rand_hex_string(6), suffix);
        final_dir = runs_base_dir.join(&run_id);
    }

    if let Err(e) = tokio::fs::rename(run_dir, &final_dir).await {
        warn!(
            "Failed to finalize run directory '{}' as '{}': {}",
            run_dir.display(),
            final_dir.display(),
            e
        );
        return (
            run_dir
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string(),
            run_dir.to_path_buf(),
        );
    }

    (run_id, final_dir)
}

/// Orchestrates the asynchronous, semaphore-guarded LaTeX compilation pipeline.
///
/// Spawns a background task to compile the target LaTeX project using either Tectonic or latexmk,
/// manages active compilation queues/priority levels, cleans stale build run histories, parses compiler warnings
/// and errors in real-time, executes post-processing compression (Ghostscript), and publishes SSE status reports.
///
/// # Arguments
///
/// * `state` - The shared global application state context.
/// * `name` - The target project name to compile.
/// * `priority` - The scheduling priority (e.g. `Auto` for watchers vs `Manual` for user clicks).
/// * `label` - Human-readable label denoting the source trigger (e.g. `"Watcher"`, `"Manual"`).
pub async fn run_build(
    state: AppState,
    name: String,
    priority: BuildPriority,
    label: &'static str,
) {
    if !is_valid_project_name(&name) {
        warn!("Invalid project name in run_build: {}", name);
        return;
    }

    info!("Build initiated [{}] for {}", label, name);

    let sess_arc = get_or_create_session(&state, &name).await;
    let mut sess = sess_arc.lock().await;

    // Check if we should cancel the running build:
    if let Some(curr_prio) = sess.current_priority {
        if priority < curr_prio {
            debug!(
                "Discarding lower priority build request [{}] for {} since a higher priority build is running.",
                label, name
            );
            return;
        }
    }

    cancel_session(&mut sess).await;
    sess.build_generation = sess.build_generation.saturating_add(1);
    let generation = sess.build_generation;
    sess.current_priority = Some(priority);

    let (cfg, raw_cfg) = read_cfg(&state.root, &name).await;

    if !is_valid_entrypoint(&cfg.entrypoint) {
        warn!("Invalid entrypoint for {}: {}", name, cfg.entrypoint);
        let _ = sess.tx.send(events::log(
            "err",
            format!(
                "Invalid entrypoint: {}. Must be a .tex file.",
                cfg.entrypoint
            ),
        ));
        let _ = sess.tx.send(events::status("Error"));
        sess.current_priority = None;
        if let Err(e) =
            record_build_history(&state, history_entry(&name, label, 0.0, BuildStatus::Error)).await
        {
            warn!("Failed to persist build history: {}", e);
        }
        return;
    }

    let proj_dir = state
        .root
        .join(&name)
        .canonicalize()
        .unwrap_or_else(|_| state.root.join(&name));
    let entrypoint = cfg.entrypoint.clone();
    let tx = sess.tx.clone();
    let sess_arc_c = sess_arc.clone();
    let semaphore = state.build_semaphore.clone();
    let timeout_duration = Duration::from_secs(state.config.build_timeout_mins * 60);

    let run_timestamp = run_timestamp();
    let pending_run_id = format!("{}_P", run_timestamp);

    sess.task = Some(tokio::spawn(async move {
        let _guard = BuildGuard {
            sess_arc: sess_arc_c.clone(),
            generation,
        };

        debug!("Waiting for build semaphore");
        let _permit = match semaphore.try_acquire() {
            Ok(p) => {
                debug!("Semaphore acquired immediately");
                p
            }
            Err(_) => {
                info!("Waiting in build queue for {}", name);
                let _ = tx.send(events::log(
                    "warn",
                    "Waiting for build slot (concurrent limit)...",
                ));
                match semaphore.acquire().await {
                    Ok(permit) => permit,
                    Err(e) => {
                        warn!("Build semaphore closed for {}: {}", name, e);
                        let _ = tx.send(events::status("Error"));
                        return;
                    }
                }
            }
        };

        debug!("Starting build task for {}", name);
        info!("Build started: {} ({})", name, entrypoint);
        let _ = tx.send(events::status(label));
        let start_time = Instant::now();

        let runs_base_dir = proj_dir.join("build").join("runs");
        let run_dir = runs_base_dir.join(&pending_run_id);
        let stem = entrypoint.replace(".tex", "");
        let pdf_path = run_dir.join(format!("{}.pdf", stem));

        if toml::from_str::<crate::config::Config>(&raw_cfg).is_err() {
            warn!("Invalid EasyTex.toml for {}, using defaults", name);
            let _ = tx.send(events::log(
                "err",
                "EasyTex.toml is invalid. Using defaults.",
            ));
        }

        // 1. Try to recover/copy intermediate files from the previous successful run to make this build incremental.
        if let Some((prev_run, prev_run_dir)) = latest_success_run_dir(&runs_base_dir).await {
            debug!("Copying intermediate files from previous run: {}", prev_run);
            if let Ok(mut entries) = tokio::fs::read_dir(&prev_run_dir).await {
                let _ = tokio::fs::create_dir_all(&run_dir).await;
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                            if ext != "pdf" {
                                let target = run_dir.join(entry.file_name());
                                let _ = tokio::fs::copy(&path, &target).await;
                            }
                        }
                    }
                }
            }
        }

        // Ensure run_dir exists:
        if let Err(e) = tokio::fs::create_dir_all(&run_dir).await {
            warn!("Failed to create run dir for {}: {}", name, e);
            let _ = tx.send(events::log(
                "err",
                format!("Failed to create run dir: {}", e),
            ));
            let _ = tx.send(events::status("Idle"));
            return;
        }

        debug!("Preparing build directory structure for {}", name);
        let mut walker = vec![proj_dir.clone()];
        while let Some(d) = walker.pop() {
            if let Ok(mut entries) = tokio::fs::read_dir(&d).await {
                while let Ok(Some(e)) = entries.next_entry().await {
                    if let Ok(ft) = e.file_type().await {
                        if ft.is_dir() {
                            let p = e.path();
                            let n = e.file_name();
                            if n != "build"
                                && n != ".git"
                                && n != "target"
                                && n != ".vscode"
                                && n != ".zed"
                            {
                                if let Ok(rel) = p.strip_prefix(&proj_dir) {
                                    let _ = tokio::fs::create_dir_all(run_dir.join(rel)).await;
                                }
                                walker.push(p);
                            }
                        }
                    }
                }
            }
        }

        async fn run_cmd(
            cmd: &mut Command,
            tx: &broadcast::Sender<String>,
            sess_arc: &Arc<Mutex<Session>>,
            timeout_duration: Duration,
            generation: u64,
        ) -> CommandOutcome {
            process::prepare_command(cmd);

            match cmd.spawn() {
                Err(e) => {
                    warn!("Failed to spawn process: {}", e);
                    let _ = tx.send(events::log(
                        "err",
                        format!("Failed to spawn process: {}", e),
                    ));
                    CommandOutcome::Failed
                }
                Ok(mut child) => {
                    let tracked_process = match process::track_child(&child) {
                        Ok(process) => process,
                        Err(e) => {
                            warn!("Failed to track process tree: {}", e);
                            let _ = tx.send(events::log(
                                "err",
                                format!("Failed to track process tree: {}", e),
                            ));
                            let _ = child.kill().await;
                            return CommandOutcome::Failed;
                        }
                    };
                    let pid = tracked_process.as_ref().map(|process| process.pid());
                    debug!("Build process started with PID: {:?}", pid);
                    {
                        let mut sess = sess_arc.lock().await;
                        if sess.build_generation != generation {
                            drop(sess);
                            if let Some(process) = tracked_process {
                                process.terminate().await;
                            } else {
                                let _ = child.kill().await;
                            }
                            return CommandOutcome::Failed;
                        }
                        sess.process = tracked_process;
                        sess.last_accessed = Instant::now();
                    }

                    let Some(stdout) = child.stdout.take() else {
                        warn!("Build process stdout unavailable");
                        return CommandOutcome::Failed;
                    };
                    let Some(stderr) = child.stderr.take() else {
                        warn!("Build process stderr unavailable");
                        return CommandOutcome::Failed;
                    };
                    let tx_c = tx.clone();
                    let h1 = tokio::spawn(async move {
                        let mut lines = tokio::io::BufReader::new(stdout).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            emit_log_line(&tx_c, log_level_for_stdout(&line), line);
                        }
                    });
                    let tx_c2 = tx.clone();
                    let h2 = tokio::spawn(async move {
                        let mut lines = tokio::io::BufReader::new(stderr).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            emit_log_line(&tx_c2, log_level_for_stderr(&line), line);
                        }
                    });

                    let mut timed_out = false;
                    let status = match tokio::time::timeout(timeout_duration, child.wait()).await {
                        Ok(status) => status,
                        Err(_) => {
                            timed_out = true;
                            warn!("Build timeout reached - killing process");
                            let _ = tx.send(events::log(
                                "err",
                                format!(
                                    "Timeout reached ({}s). Killing process group.",
                                    timeout_duration.as_secs()
                                ),
                            ));
                            let process = {
                                let mut sess = sess_arc.lock().await;
                                if sess.build_generation == generation {
                                    sess.process.take()
                                } else {
                                    None
                                }
                            };
                            if let Some(process) = process {
                                process.terminate().await;
                            } else {
                                let _ = child.kill().await;
                            }
                            child.wait().await
                        }
                    };

                    let _ = h1.await;
                    let _ = h2.await;
                    {
                        let mut sess = sess_arc.lock().await;
                        if sess.build_generation == generation {
                            sess.process = None;
                            sess.last_accessed = Instant::now();
                        }
                    }
                    if timed_out {
                        return CommandOutcome::TimedOut;
                    }
                    match status {
                        Ok(s) => {
                            if s.success() {
                                debug!("Build process completed successfully");
                            } else {
                                warn!("Build process exited with status: {}", s);
                            }
                            if s.success() {
                                CommandOutcome::Success
                            } else {
                                CommandOutcome::Failed
                            }
                        }
                        Err(e) => {
                            warn!("Build process wait error: {}", e);
                            let _ = tx.send(events::log("err", format!("Wait error: {}", e)));
                            CommandOutcome::Failed
                        }
                    }
                }
            }
        }

        let _ = tx.send(events::log("dim", format!("Building {}...", entrypoint)));
        debug!("Compilation engine starting");

        #[cfg(not(feature = "latexmk"))]
        let build_outcome = {
            info!("Using Tectonic engine");
            let _ = tx.send(events::log("dim", "Tectonic engine active..."));
            let mut cmd = Command::new("tectonic");
            cmd.args([
                "-X",
                "compile",
                &entrypoint,
                "--synctex",
                "--outdir",
                &run_dir.to_string_lossy(),
            ])
            .current_dir(&proj_dir);
            run_cmd(&mut cmd, &tx, &sess_arc_c, timeout_duration, generation).await
        };

        #[cfg(feature = "latexmk")]
        let build_outcome = {
            info!("Using Latexmk fallback");
            let _ = tx.send(events::log(
                "warn",
                "Latexmk feature active. Using Latexmk fallback...",
            ));
            let mut cmd = Command::new("texfot");
            cmd.args([
                "latexmk",
                "-pdf",
                "-interaction=nonstopmode",
                "-synctex=1",
                "-file-line-error",
                "-recorder",
                "-f",
                &format!("-outdir={}", run_dir.to_string_lossy()),
                &entrypoint,
            ])
            .current_dir(&proj_dir);
            if state.config.allow_shell_escape {
                cmd.arg("-shell-escape");
            }

            let texinputs = format!("{}:{}/build:", proj_dir.display(), proj_dir.display());
            cmd.env("TEXINPUTS", texinputs);
            run_cmd(&mut cmd, &tx, &sess_arc_c, timeout_duration, generation).await
        };

        let log_file = run_dir.join(format!("{}.log", stem));
        if log_file.exists() && state.capabilities.log_analysis {
            debug!("Analyzing build warnings");
            let mut analyzer = Command::new("texloganalyser");
            analyzer
                .args(["-w", log_file.to_str().unwrap_or_default()])
                .current_dir(&proj_dir);
            let _ = tx.send(events::log("dim", "Analyzing warnings..."));
            let _ = run_cmd(
                &mut analyzer,
                &tx,
                &sess_arc_c,
                timeout_duration,
                generation,
            )
            .await;
        } else if log_file.exists() {
            debug!("Skipping texloganalyser because it is not installed");
        }

        if build_outcome != CommandOutcome::Success {
            let status = build_outcome.build_status();
            warn!("Build ended with status {} for {}", status.as_str(), name);
            let message = if status == BuildStatus::TimedOut {
                "Build timed out. Check logs for details."
            } else {
                "Build failed. Check logs for details."
            };
            let _ = tx.send(events::log("err", message));
            let _ = tx.send(events::status(status.as_str()));
            let (failed_run_id, _) = finalize_run_dir(
                &run_dir,
                &runs_base_dir,
                &run_timestamp,
                status.run_suffix().unwrap_or('F'),
            )
            .await;
            if let Err(e) = record_build_history(
                &state,
                history_entry(&name, label, start_time.elapsed().as_secs_f32(), status),
            )
            .await
            {
                warn!("Failed to persist build history: {}", e);
            }
            clean_old_runs(&proj_dir, Some(&failed_run_id)).await;
            return;
        }

        let synctex_path = run_dir.join(format!("{}.synctex.gz", stem));
        if synctex_path.exists() {
            debug!("SyncTeX data ready: {}", synctex_path.display());
        } else {
            warn!("SyncTeX data missing for {}", name);
            let _ = tx.send(events::log(
                "warn",
                "SyncTeX data not generated; source/PDF navigation may be unavailable.",
            ));
        }

        let build_duration = start_time.elapsed().as_secs_f32();
        info!("Build succeeded in {:.1}s", build_duration);

        if state.config.compress_pdf && state.capabilities.pdf_compression {
            debug!("Compressing PDF");
            let compressed_path = run_dir.join("compressed.pdf");
            let mut gs = Command::new("gs");
            gs.args([
                "-sDEVICE=pdfwrite",
                "-dCompatibilityLevel=1.4",
                "-dPDFSETTINGS=/screen",
                "-dNOPAUSE",
                "-dQUIET",
                "-dBATCH",
                &format!("-sOutputFile={}", compressed_path.display()),
                &pdf_path.display().to_string(),
            ])
            .current_dir(&run_dir);
            match run_cmd(&mut gs, &tx, &sess_arc_c, timeout_duration, generation).await {
                CommandOutcome::Success => {
                    if tokio::fs::rename(&compressed_path, &pdf_path).await.is_ok() {
                        info!("PDF compressed successfully");
                        let _ = tx.send(events::log("ok", "PDF compressed successfully."));
                    } else {
                        warn!("Failed to replace original PDF with compressed PDF");
                        let _ = tx.send(events::log(
                            "warn",
                            "PDF compression finished, but replacing the original PDF failed.",
                        ));
                    }
                }
                CommandOutcome::TimedOut => {
                    warn!("PDF compression timed out");
                    let _ = tx.send(events::log(
                        "warn",
                        "PDF compression timed out; keeping the uncompressed PDF.",
                    ));
                }
                CommandOutcome::Failed => {
                    warn!("PDF compression failed");
                    let _ = tx.send(events::log(
                        "warn",
                        "PDF compression failed; keeping the uncompressed PDF.",
                    ));
                }
            }
        } else if state.config.compress_pdf {
            warn!("PDF compression requested but Ghostscript (gs) is not installed");
            let _ = tx.send(events::log(
                "warn",
                "PDF compression skipped: Ghostscript (gs) is not installed.",
            ));
        }

        let pdf_size = get_pdf_size(&pdf_path).await;
        let words = get_word_count(&proj_dir, &entrypoint).await;
        debug!("Build stats - Size: {}, Words: {}", pdf_size, words);

        let (final_run_id, _) = finalize_run_dir(
            &run_dir,
            &runs_base_dir,
            &run_timestamp,
            BuildStatus::Success.run_suffix().unwrap_or('S'),
        )
        .await;
        let _ = tokio::fs::remove_file(proj_dir.join("build").join("preview.json")).await;
        let _ = tokio::fs::remove_file(proj_dir.join("build").join("preview.json.tmp")).await;

        let _ = tx.send(events::log(
            "ok",
            format!("Build complete in {:.1}s", build_duration),
        ));
        let _ = tx.send(events::pdf_reload());
        let _ = tx.send(events::stats(
            format!("{:.1}s", build_duration),
            pdf_size,
            words,
        ));
        let _ = tx.send(events::status("Idle"));

        if let Err(e) = record_build_history(
            &state,
            history_entry(&name, label, build_duration, BuildStatus::Success),
        )
        .await
        {
            warn!("Failed to persist build history: {}", e);
        } else {
            info!("Build history updated");
        }

        // Clean old runs in the background
        let proj_dir_c = proj_dir.clone();
        tokio::spawn(async move {
            clean_old_runs(&proj_dir_c, Some(&final_run_id)).await;
        });

        info!("Build completed for {}", name);
    }));
}

#[cfg(test)]
mod tests {
    use super::clean_old_runs;

    #[tokio::test]
    async fn clean_old_runs_keeps_at_least_three_success_dirs() {
        let root = std::env::temp_dir().join(format!(
            "easytex-run-history-{}-{}",
            std::process::id(),
            crate::utils::rand_hex_string(8)
        ));
        let runs_dir = root.join("build").join("runs");
        tokio::fs::create_dir_all(&runs_dir).await.unwrap();

        for name in [
            "20260101-000000-000000000_S",
            "20260101-000001-000000000_S",
            "20260101-000002-000000000_S",
            "20260101-000003-000000000_F",
            "20260101-000004-000000000_F",
            "20260101-000005-000000000_F",
            "20260101-000006-000000000_F",
            "20260101-000007-000000000_F",
            "20260101-000008-000000000_F",
            "20260101-000009-000000000_F",
            "20260101-000010-000000000_F",
            "20260101-000011-000000000_F",
        ] {
            tokio::fs::create_dir_all(runs_dir.join(name))
                .await
                .unwrap();
        }

        clean_old_runs(&root, None).await;

        let mut entries = tokio::fs::read_dir(&runs_dir).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name().to_string_lossy().to_string());
        }

        assert_eq!(names.len(), 10);
        assert_eq!(names.iter().filter(|name| name.ends_with("_S")).count(), 3);

        let _ = tokio::fs::remove_dir_all(&root).await;
    }
}
