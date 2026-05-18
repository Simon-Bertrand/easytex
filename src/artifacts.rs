//! # Build Artifacts & Preview Recovery Module
//!
//! This module coordinates the indexing and retrieval of successful PDF preview artifacts
//! within the sandboxed project directories.
//!
//! EasyTex maintains build runs inside a segmented directories history (located in `build/runs/`).
//! This module resolves paths to compiled PDFs and SyncTeX files, manages backward-compatible
//! "legacy" single-build preview fallbacks, parses timestamps from compile run hashes,
//! and retrieves file modification metadata.

use std::{
    path::{Path as FsPath, PathBuf},
    time::UNIX_EPOCH,
};

/// Structured representation of a completed, successful preview compile run.
#[derive(Debug, Clone)]
pub struct PreviewInfo {
    /// Unique identifier containing the compile timestamp and run status suffix.
    pub run: String,
    /// Absolute path to the compiled preview PDF on the host filesystem.
    pub pdf_path: PathBuf,
    /// Absolute path to the companion `.synctex.gz` file on the host filesystem.
    pub synctex_path: PathBuf,
    /// Completion time represented as epoch millisecond timestamp.
    pub built_at_ms: u64,
}

/// Retrieves the filesystem paths of the compiled PDF and SyncTeX file for the latest successful run.
///
/// Returns `None` if no successful runs are recorded or readable.
///
/// # Arguments
///
/// * `project_dir` - Path reference to the target project on disk.
/// * `entrypoint` - Entry point LaTeX filename (e.g. `"main.tex"`).
pub async fn get_preview_paths(
    project_dir: &FsPath,
    entrypoint: &str,
) -> Option<(PathBuf, PathBuf)> {
    latest_success_preview(project_dir, entrypoint)
        .await
        .map(|preview| (preview.pdf_path, preview.synctex_path))
}

/// Searches the project directory and returns the absolute latest successful preview run details.
pub async fn latest_success_preview(project_dir: &FsPath, entrypoint: &str) -> Option<PreviewInfo> {
    success_previews(project_dir, entrypoint)
        .await
        .into_iter()
        .next()
}

/// Discovers, reads, and returns all successful run entries in the project's history.
///
/// Automatically falls back to checking legacy single-build preview directories if structured
/// run directories do not exist.
///
/// # Arguments
///
/// * `project_dir` - Safe path pointing to the project root.
/// * `entrypoint` - LaTeX entry point document name (used to locate the `.pdf` and `.synctex.gz`).
pub async fn success_previews(project_dir: &FsPath, entrypoint: &str) -> Vec<PreviewInfo> {
    let build_dir = project_dir.join("build");
    let runs_dir = build_dir.join("runs");
    let stem = entrypoint.replace(".tex", "");
    let mut entries = match tokio::fs::read_dir(&runs_dir).await {
        Ok(entries) => entries,
        Err(_) => {
            return legacy_preview(&build_dir, &stem)
                .await
                .into_iter()
                .collect()
        }
    };

    let mut runs = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(file_type) = entry.file_type().await else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let run = entry.file_name().to_string_lossy().to_string();
        if !valid_success_run(&run) {
            continue;
        }
        if let Some(preview) = preview_from_run(&stem, run, entry.path()).await {
            runs.push(preview);
        }
    }

    runs.sort_by(|a, b| b.run.cmp(&a.run));
    if runs.is_empty() {
        legacy_preview(&build_dir, &stem)
            .await
            .into_iter()
            .collect()
    } else {
        runs
    }
}

/// Reconstitutes a successful preview using a specific run ID directory name.
pub async fn success_preview_by_run(
    project_dir: &FsPath,
    entrypoint: &str,
    run: &str,
) -> Option<PreviewInfo> {
    if !valid_success_run(run) {
        return None;
    }
    let stem = entrypoint.replace(".tex", "");
    let run_dir = project_dir.join("build").join("runs").join(run);
    preview_from_run(&stem, run.to_string(), run_dir).await
}

/// Validates whether a run identifier matches the required alphanumeric success formatting.
///
/// The run ID must end with `_S` (indicating Success) and be at most 64 characters long to prevent
/// path manipulation exploits.
pub fn valid_success_run(run: &str) -> bool {
    run.ends_with("_S")
        && run.len() <= 64
        && run
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

/// Builds a `PreviewInfo` from a validated run directory on the filesystem.
async fn preview_from_run(stem: &str, run: String, run_dir: PathBuf) -> Option<PreviewInfo> {
    let pdf_path = run_dir.join(format!("{}.pdf", stem));
    if tokio::fs::metadata(&pdf_path).await.is_err() {
        return None;
    }

    let synctex_path = run_dir.join(format!("{}.synctex.gz", stem));
    let built_at_ms = run_built_at_ms(&run)
        .or_else(|| modified_at_ms(&pdf_path))
        .unwrap_or(0);
    Some(PreviewInfo {
        run,
        pdf_path,
        synctex_path,
        built_at_ms,
    })
}

/// Fallback preview search checking if compilation has occurred in the flat `build/` directory.
async fn legacy_preview(build_dir: &FsPath, stem: &str) -> Option<PreviewInfo> {
    let pdf_path = build_dir.join(format!("{}.pdf", stem));
    if tokio::fs::metadata(&pdf_path).await.is_err() {
        return None;
    }
    let synctex_path = build_dir.join(format!("{}.synctex.gz", stem));
    Some(PreviewInfo {
        run: "legacy".into(),
        built_at_ms: modified_at_ms(&pdf_path).unwrap_or(0),
        pdf_path,
        synctex_path,
    })
}

/// Parses the microsecond UTC timestamp embedded in standard run ID strings to get millisecond epoch times.
fn run_built_at_ms(run: &str) -> Option<u64> {
    let timestamp = run.strip_suffix("_S")?;
    let parsed = chrono::NaiveDateTime::parse_from_str(timestamp, "%Y%m%d-%H%M%S-%f").ok()?;
    let utc = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(parsed, chrono::Utc);
    Some(utc.timestamp_millis().max(0) as u64)
}

/// Standard file metadata lookup returning last modified time as epoch milliseconds.
fn modified_at_ms(path: &FsPath) -> Option<u64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
}

#[cfg(test)]
mod tests {
    use super::valid_success_run;

    #[test]
    fn valid_success_run_accepts_only_safe_success_ids() {
        assert!(valid_success_run("20260101-010203-000000000_S"));
        assert!(!valid_success_run("20260101-010203-000000000_F"));
        assert!(!valid_success_run("../20260101-010203-000000000_S"));
        assert!(!valid_success_run("x".repeat(65).as_str()));
    }
}
