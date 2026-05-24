//! # Data Transfer Objects (DTO) Module
//!
//! This module defines the standardized, strongly typed JSON structures exchanged between
//! the Rust backend (Axum) and the SolidJS dashboard frontend.
//!
//! All types in this file derive `ts_rs::TS` to automatically generate corresponding TypeScript
//! interfaces inside `frontend/src/bindings/` during compilation. This guarantees complete
//! compile-time API safety and structural synchronization across the entire client-server boundary.

use serde::Serialize;
use ts_rs::TS;

/// Represents the execution state of a specific project.
#[derive(Serialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub struct ProjectStatusResponse {
    /// The project's active state. Typically `"building"` or `"idle"`.
    pub status: String,
}

/// List of known projects served by the current EasyTex root directory.
#[derive(Serialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub struct ProjectsResponse {
    /// Project names sorted lexicographically.
    pub projects: Vec<String>,
}

/// Dynamic information about the latest successful PDF compilation of a project.
#[derive(Serialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub struct PreviewResponse {
    /// Unique identifier for the compile run (e.g. `"20260518-172100-123456_S"`).
    pub run: String,
    /// Absolute timestamp in milliseconds when this PDF was generated.
    #[ts(type = "number")]
    pub built_at_ms: u64,
    /// File size in bytes of the generated PDF.
    #[ts(type = "number")]
    pub pdf_size_bytes: u64,
}

/// Metadata about a single historic compile run.
#[derive(Serialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub struct BuildArtifactResponse {
    /// Unique run ID of the historic compilation.
    pub run: String,
    /// Absolute timestamp in milliseconds when this build was completed.
    #[ts(type = "number")]
    pub built_at_ms: u64,
    /// Size of the resulting PDF in bytes.
    #[ts(type = "number")]
    pub pdf_size_bytes: u64,
}

/// Envelopes the raw string content of a project's `EasyTex.toml` configuration.
#[derive(Serialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub struct ConfigResponse {
    /// Raw TOML settings file content.
    pub raw: String,
}

/// Struct containing the raw contents of a project file.
#[derive(Serialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub struct FileResponse {
    /// The complete textual content of the requested document.
    pub content: String,
    /// The relative, sandboxed path of the requested document from the project root.
    pub path: String,
}

/// Lists all editable files within a project directory.
#[derive(Serialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub struct FileListResponse {
    /// Vector of relative paths for all files with safe, editable extensions.
    pub files: Vec<String>,
    /// Indicates whether the search was completed successfully, or truncated due to limits.
    pub complete: bool,
}

/// Return payload for SyncTeX PDF-to-Source editing lookup request.
#[derive(Serialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub struct SynctexEditResponse {
    /// Relative path of the corresponding LaTeX file (e.g., `"main.tex"`).
    pub file: String,
    /// Line number (1-based index) matching the clicked coordinate.
    pub line: u32,
    /// Column number (1-based index) matching the clicked coordinate.
    pub column: u32,
}

/// Return payload for SyncTeX Source-to-PDF viewing coordinates lookup request.
#[derive(Serialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub struct SynctexViewResponse {
    /// Target page number (1-based index) inside the compiled PDF.
    pub page: u32,
    /// Exact horizontal page coordinate (in PDF points) matching the cursor location.
    pub x: f32,
    /// Exact vertical page coordinate (in PDF points) matching the cursor location.
    pub y: f32,
}

/// Structural report containing chktex style compiler or linter diagnostic results.
#[derive(Serialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub struct LintResponse {
    /// True if the document has passed all check-points with zero warnings.
    pub ok: bool,
    /// Command-line exit code of the underlying linter process (`chktex`).
    pub status: Option<i32>,
    /// Standard output buffer capturing linter diagnostics.
    pub stdout: String,
    /// Standard error buffer capturing process flags or configuration warnings.
    pub stderr: String,
}

/// Operational counters exposed to authenticated administrators.
#[derive(Serialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub struct AdminMetricsResponse {
    /// Total number of recognized project directories under the configured root.
    pub projects: usize,
    /// Number of in-memory project sessions currently retained.
    pub active_sessions: usize,
    /// Number of sessions with a tracked compiler process.
    pub active_builds: usize,
    /// Available concurrent build permits at the time of sampling.
    pub available_build_slots: usize,
    /// Configured concurrent build capacity.
    pub max_concurrent_builds: usize,
    /// Number of persisted build history entries currently loaded.
    pub history_entries: usize,
    /// Whether mutating routes are disabled.
    pub read_only: bool,
    /// Whether bearer auth is mandatory for admin/mutating routes.
    pub auth_required: bool,
}
