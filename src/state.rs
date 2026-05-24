//! # Thread-Safe Application State and Sessions Module
//!
//! This module coordinates the global in-memory state of the EasyTex server.
//! It maintains thread-safe mapping for active project compilation sessions, encapsulates global
//! configuration parameters, tracks the build queue through asynchronous Semaphores to enforce concurrency limits,
//! and persists compiled run histories to the host filesystem.

use crate::{
    capabilities::RuntimeCapabilities, config::GlobalConfig, frontend_assets::FrontendAssets,
    process::RunningProcess,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{broadcast, Mutex};
use tracing::{debug, info, warn};

/// Global, shared, thread-safe application context.
///
/// Cloned cheaply across Axum route handlers to distribute database connections, configuration details,
/// active watch/build queues, and state telemetry.
#[derive(Clone)]
pub struct AppState {
    /// Sandbox root directory containing LaTeX project sub-folders.
    pub root: PathBuf,
    /// In-memory cache holding active watch sessions and telemetry streams.
    pub sessions: Arc<Mutex<HashMap<String, Arc<Mutex<Session>>>>>,
    /// System-wide global configuration settings.
    pub config: Arc<GlobalConfig>,
    /// Limit-controlling semaphore protecting system resources from concurrent build overloading.
    pub build_semaphore: Arc<tokio::sync::Semaphore>,
    /// Historic compilation records of all processed projects.
    pub history: Arc<Mutex<Vec<BuildHistoryEntry>>>,
    /// Storage path pointing to the persistent JSON history ledger.
    pub history_path: PathBuf,
    /// Detected host-level compilation capabilities.
    pub capabilities: RuntimeCapabilities,
    /// Frontend bundle decompressed once from the embedded archive at process startup.
    pub frontend_assets: Arc<FrontendAssets>,
}

/// Constant limit representing the maximum size of historic runs saved inside the JSON ledger.
const MAX_BUILD_HISTORY_ENTRIES: usize = 10;
/// Minimum threshold of successful build directories kept from garbage collection.
const MIN_SUCCESS_HISTORY_ENTRIES: usize = 3;

/// Entry record capturing details of a historic LaTeX compile task.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BuildHistoryEntry {
    /// Target project name.
    pub project: String,
    /// Formatted completion timestamp.
    pub timestamp: String,
    /// Active compilation duration in seconds.
    pub duration: f32,
    /// Stable execution result state (e.g. `"Success"`, `"Failed"`, `"Error"`).
    pub status: String,
    /// Request source label (e.g., `"Manual Build"`, `"Auto-Build"`).
    pub label: String,
}

/// Unified compilation outcome states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BuildStatus {
    /// Task is resting inside the build semaphore slot queue.
    Queued,
    /// Process is spawned and currently translating LaTeX macros.
    Running,
    /// Executed with status code `0`, outputting a preview PDF.
    Success,
    /// Process failed, returned a non-zero exit code.
    Failed,
    /// User manually stopped compilation.
    Cancelled,
    /// Execution exceeded configured runtime thresholds.
    TimedOut,
    /// Spawning sub-process or writing folder structures crashed the engine.
    Error,
}

impl BuildStatus {
    /// Serializes the status into a stable, frontend-friendly string identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Running => "Running",
            Self::Success => "Success",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::TimedOut => "TimedOut",
            Self::Error => "Error",
        }
    }

    /// Translates execution results into run directory name suffixes.
    ///
    /// Suffix `S` is used for success, while `F` indicates standard failures.
    pub fn run_suffix(self) -> Option<char> {
        match self {
            Self::Success => Some('S'),
            Self::Failed | Self::TimedOut | Self::Error => Some('F'),
            _ => None,
        }
    }
}

/// Request priority ordering representing queue insertion hierarchies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BuildPriority {
    /// Spawned by automated filesystem watchers on file changes.
    Auto = 0,
    /// Spawned by manual user intervention via the dashboard interface.
    Manual = 1,
}

/// An active project session tracking build execution, file watchers, and SSE log subscribers.
pub struct Session {
    /// Stream transmitter broadcasting compile outputs to listening browsers.
    pub tx: broadcast::Sender<String>,
    /// Active spawn handle managing LaTeX background compilation.
    pub task: Option<tokio::task::JoinHandle<()>>,
    /// Tracked compiler process tree, including native OS cancellation handles.
    pub process: Option<RunningProcess>,
    /// Operating system directory file watcher monitoring workspace changes.
    pub _watcher: Option<Box<dyn notify::Watcher + Send>>,
    /// Absolute instant when the session last performed API work. Used for TTL calculations.
    pub last_accessed: Instant,
    /// Current priority of the running compilation task.
    pub current_priority: Option<BuildPriority>,
    /// Monotonic token used to avoid stale build tasks clearing newer session state.
    pub build_generation: u64,
}

/// Securely terminates any active subprocesses and handles abort routines on the active session thread.
pub async fn cancel_session(sess: &mut Session) {
    if let Some(h) = sess.task.take() {
        h.abort();
        debug!("Aborted build task");
    }
    if let Some(process) = sess.process.take() {
        info!("Terminating process tree for PID: {}", process.pid());
        process.terminate().await;
        debug!("Process terminated");
    }
}

/// Obtains an existing active session, or constructs a new telemetry and watch channel.
pub async fn get_or_create_session(state: &AppState, name: &str) -> Arc<Mutex<Session>> {
    let mut map = state.sessions.lock().await;
    let is_new = !map.contains_key(name);

    let sess = map
        .entry(name.to_string())
        .or_insert_with(|| {
            let (tx, _) = broadcast::channel(2048);
            let now = Instant::now();
            if is_new {
                debug!("Creating new session for {}", name);
            }
            Arc::new(Mutex::new(Session {
                tx,
                task: None,
                process: None,
                _watcher: None,
                last_accessed: now,
                current_priority: None,
                build_generation: 0,
            }))
        })
        .clone();

    if !is_new {
        debug!("Reusing existing session for {}", name);
    }
    sess
}

/// Asynchronously loads build history data from the designated storage path.
///
/// Automatically handles corruption by moving problematic files aside and constructing a fresh vector.
pub async fn load_build_history(path: &PathBuf) -> Vec<BuildHistoryEntry> {
    match tokio::fs::read_to_string(path).await {
        Ok(raw) => match serde_json::from_str::<Vec<BuildHistoryEntry>>(&raw) {
            Ok(mut history) => {
                prune_build_history(&mut history);
                history
            }
            Err(e) => {
                warn!("Failed to parse build history '{}': {}", path.display(), e);
                let bad_path = path.with_extension(format!("json.bad.{}", timestamp_suffix()));
                if let Err(rename_err) = tokio::fs::rename(path, &bad_path).await {
                    warn!(
                        "Failed to move corrupt build history '{}' aside: {}",
                        path.display(),
                        rename_err
                    );
                }
                Vec::new()
            }
        },
        Err(_) => Vec::new(),
    }
}

/// Asynchronously saves build history records to disk using an atomic rename sequence.
pub async fn save_build_history(path: &PathBuf, history: &[BuildHistoryEntry]) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp_path = path.with_extension(format!(
        "json.tmp.{}.{}",
        std::process::id(),
        timestamp_suffix()
    ));
    let bytes = serde_json::to_vec_pretty(history)?;
    tokio::fs::write(&tmp_path, bytes).await?;
    tokio::fs::rename(&tmp_path, path).await?;
    Ok(())
}

/// Adds a new entry to the historical log, prunes old data, and saves state to disk.
pub async fn record_build_history(state: &AppState, entry: BuildHistoryEntry) -> Result<()> {
    let mut history = state.history.lock().await;
    history.push(entry);
    prune_build_history(&mut history);
    save_build_history(&state.history_path, &history).await
}

/// Generates a simple timestamp suffix using current nanoseconds since Unix epoch.
fn timestamp_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

/// Truncates the compile records list to prevent infinite growth.
///
/// Ensures a set of successful builds are locked against aggressive GC.
fn prune_build_history(history: &mut Vec<BuildHistoryEntry>) {
    if history.len() <= MAX_BUILD_HISTORY_ENTRIES {
        return;
    }

    let success_indices = history
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| (entry.status == "Success").then_some(index))
        .collect::<Vec<_>>();
    let required_successes = MIN_SUCCESS_HISTORY_ENTRIES.min(success_indices.len());

    let mut keep = HashSet::new();
    for index in success_indices.into_iter().rev().take(required_successes) {
        keep.insert(index);
    }

    for index in (0..history.len()).rev() {
        if keep.len() >= MAX_BUILD_HISTORY_ENTRIES {
            break;
        }
        keep.insert(index);
    }

    let next = history
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| keep.contains(&index).then_some(entry.clone()))
        .collect();
    *history = next;
}

/// Periodically triggered routine identifying and reclaiming expired idle sessions.
pub async fn cleanup_expired_sessions(state: &AppState, ttl_secs: u64) {
    let mut map = state.sessions.lock().await;
    let now = Instant::now();
    let ttl = Duration::from_secs(ttl_secs);

    let mut to_remove = Vec::new();
    for (name, sess_arc) in map.iter() {
        if let Ok(sess) = sess_arc.try_lock() {
            if now.duration_since(sess.last_accessed) > ttl {
                to_remove.push(name.clone());
            }
        }
    }

    if !to_remove.is_empty() {
        info!("Cleaning {} expired sessions", to_remove.len());
    }

    for name in to_remove {
        if let Some(sess_arc) = map.remove(&name) {
            info!("Expired session removed: {}", name);
            let mut sess = sess_arc.lock().await;
            cancel_session(&mut sess).await;
        }
    }

    if map.len() > 1000 {
        warn!("Large number of active sessions: {}", map.len());
    }
}

#[cfg(test)]
mod tests {
    use super::{prune_build_history, BuildHistoryEntry, BuildStatus};

    fn entry(index: usize, status: &str) -> BuildHistoryEntry {
        BuildHistoryEntry {
            project: "demo".into(),
            timestamp: format!("2026-01-01 00:00:{index:02}"),
            duration: index as f32,
            status: status.into(),
            label: "test".into(),
        }
    }

    #[test]
    fn history_keeps_ten_entries_with_three_successes_when_available() {
        let mut history = vec![
            entry(0, "Success"),
            entry(1, "Success"),
            entry(2, "Success"),
        ];
        for index in 3..16 {
            history.push(entry(index, "Error"));
        }

        prune_build_history(&mut history);

        assert_eq!(history.len(), 10);
        assert_eq!(
            history
                .iter()
                .filter(|entry| entry.status == "Success")
                .count(),
            3
        );
        assert_eq!(history.first().map(|entry| entry.duration), Some(0.0));
        assert_eq!(history.last().map(|entry| entry.duration), Some(15.0));
    }

    #[test]
    fn history_keeps_available_successes_when_fewer_than_three_exist() {
        let mut history = vec![entry(0, "Success"), entry(1, "Success")];
        for index in 2..16 {
            history.push(entry(index, "Error"));
        }

        prune_build_history(&mut history);

        assert_eq!(history.len(), 10);
        assert_eq!(
            history
                .iter()
                .filter(|entry| entry.status == "Success")
                .count(),
            2
        );
    }

    #[test]
    fn build_status_maps_to_stable_strings_and_run_suffixes() {
        assert_eq!(BuildStatus::Success.as_str(), "Success");
        assert_eq!(BuildStatus::Failed.as_str(), "Failed");
        assert_eq!(BuildStatus::Success.run_suffix(), Some('S'));
        assert_eq!(BuildStatus::Failed.run_suffix(), Some('F'));
        assert_eq!(BuildStatus::Running.run_suffix(), None);
    }
}
