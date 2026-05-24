//! # Configuration System Module
//!
//! EasyTex parses both system-wide configuration (`easytex.yaml`) and project-specific
//! settings (`EasyTex.toml`) to customize the LaTeX server's behavior.
//!
//! This module structures, merges, and validates these configuration options.
//! It supports loading settings from YAML, reading from the host environment to enable painless
//! Docker containerization, and enforcing safety thresholds (e.g. file size and count limit checks).

use serde::{Deserialize, Serialize};
use std::path::Path as FsPath;

use crate::{
    fs_safety,
    utils::{is_valid_entrypoint, MAX_CONFIG_SIZE},
};

/// System-wide global configuration settings.
///
/// Can be loaded from `easytex.yaml` and refined using environment variables. Enforces resource
/// limits and security rules.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct GlobalConfig {
    /// Port number the Axum server will listen on. Defaults to `8081`.
    pub port: u16,
    /// Parent directory housing all LaTeX projects. Defaults to the current workspace root `"."`.
    pub root_dir: String,
    /// Maximum number of compilation threads running concurrently. Defaults to `4`.
    pub max_concurrent_builds: usize,
    /// Time-to-live for inactive sessions before they are reclaimed. Defaults to `24` hours.
    pub session_ttl_hours: u64,
    /// Max allowable run duration for a single build before it is killed. Defaults to `15` minutes.
    pub build_timeout_mins: u64,
    /// Enables post-build Ghostscript (gs) size compression. Defaults to `true`.
    pub compress_pdf: bool,
    /// Optional bearer token required to access administrative dashboards.
    pub admin_token: Option<String>,
    /// Forces mutating API requests and admin routes to require a bearer token even on localhost.
    pub require_auth: bool,
    /// Allows the TeX engine to execute arbitrary shell commands via `-shell-escape`. Defaults to `false`.
    pub allow_shell_escape: bool,
    /// White-listed origins for CORS verification. Supports `"*"` for permissive open-sharing.
    pub cors_allowed_origins: Vec<String>,
    /// Network interface or host IP address to bind the server socket to (e.g., `"127.0.0.1"`).
    pub host: String,
    /// Filename of the JSON file storing historic compile metrics. Defaults to `".easytex-history.json"`.
    pub history_file: String,
    /// Maximum byte size permitted when writing/saving a project file. Defaults to `1 MB`.
    pub max_edit_file_size_bytes: usize,
    /// Maximum byte size permitted when loading/reading a project file. Defaults to `2 MB`.
    pub max_read_file_size_bytes: usize,
    /// Maximum number of documents allowed per project folder. Defaults to `5,000`.
    pub max_project_files: usize,
    /// Maximum byte size of PDF files that the server will return. Defaults to `100 MB`.
    pub max_pdf_size_bytes: u64,
    /// Forces the server to run in read-only mode, disabling all mutative actions. Defaults to `false`.
    pub read_only: bool,
}

impl Default for GlobalConfig {
    /// Constructs default configurations optimized for local responsive performance and basic security.
    fn default() -> Self {
        Self {
            port: 8081,
            root_dir: ".".into(),
            max_concurrent_builds: 4,
            session_ttl_hours: 24,
            build_timeout_mins: 15,
            compress_pdf: true,
            admin_token: None,
            require_auth: false,
            allow_shell_escape: false,
            cors_allowed_origins: Vec::new(),
            host: "127.0.0.1".into(),
            history_file: ".easytex-history.json".into(),
            max_edit_file_size_bytes: 1_000_000,
            max_read_file_size_bytes: 2_000_000,
            max_project_files: 5_000,
            max_pdf_size_bytes: 100 * 1024 * 1024,
            read_only: false,
        }
    }
}

impl GlobalConfig {
    /// Overrides configuration options using environment variables for easy deployment settings.
    pub fn apply_env(mut self) -> Self {
        if let Some(port) = std::env::var("PORT").ok().and_then(|p| p.parse().ok()) {
            self.port = port;
        }
        if let Ok(root) = std::env::var("ROOT_DIR") {
            self.root_dir = root;
        }
        if let Ok(host) = std::env::var("EASYTEX_HOST") {
            if !host.is_empty() {
                self.host = host;
            }
        }
        if let Ok(token) = std::env::var("EASYTEX_ADMIN_TOKEN") {
            self.admin_token = (!token.is_empty()).then_some(token);
        }
        if let Ok(require_auth) = std::env::var("EASYTEX_REQUIRE_AUTH") {
            self.require_auth = env_truthy(&require_auth);
        }
        if let Ok(origins) = std::env::var("EASYTEX_CORS_ALLOWED_ORIGINS") {
            self.cors_allowed_origins = origins
                .split(',')
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .map(ToOwned::to_owned)
                .collect();
        }
        if let Some(limit) = std::env::var("EASYTEX_MAX_EDIT_FILE_SIZE_BYTES")
            .ok()
            .and_then(|value| value.parse().ok())
        {
            self.max_edit_file_size_bytes = limit;
        }
        if let Some(limit) = std::env::var("EASYTEX_MAX_READ_FILE_SIZE_BYTES")
            .ok()
            .and_then(|value| value.parse().ok())
        {
            self.max_read_file_size_bytes = limit;
        }
        if let Some(limit) = std::env::var("EASYTEX_MAX_PROJECT_FILES")
            .ok()
            .and_then(|value| value.parse().ok())
        {
            self.max_project_files = limit;
        }
        if let Some(limit) = std::env::var("EASYTEX_MAX_PDF_SIZE_BYTES")
            .ok()
            .and_then(|value| value.parse().ok())
        {
            self.max_pdf_size_bytes = limit;
        }
        if let Ok(read_only) = std::env::var("EASYTEX_READ_ONLY") {
            self.read_only = env_truthy(&read_only);
        }
        self
    }

    /// Asserts validation checks and logical invariants on the parameters.
    ///
    /// # Errors
    ///
    /// Returns `Err` if any limit parameter is set to zero or contains an empty name.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.max_concurrent_builds > 0,
            "max_concurrent_builds must be greater than 0"
        );
        anyhow::ensure!(
            self.session_ttl_hours > 0,
            "session_ttl_hours must be greater than 0"
        );
        anyhow::ensure!(
            self.build_timeout_mins > 0,
            "build_timeout_mins must be greater than 0"
        );
        anyhow::ensure!(!self.host.trim().is_empty(), "host must not be empty");
        anyhow::ensure!(
            self.max_edit_file_size_bytes > 0,
            "max_edit_file_size_bytes must be greater than 0"
        );
        anyhow::ensure!(
            self.max_read_file_size_bytes > 0,
            "max_read_file_size_bytes must be greater than 0"
        );
        anyhow::ensure!(
            self.max_project_files > 0,
            "max_project_files must be greater than 0"
        );
        anyhow::ensure!(
            self.max_pdf_size_bytes > 0,
            "max_pdf_size_bytes must be greater than 0"
        );
        anyhow::ensure!(self.port > 0, "port must be greater than 0");
        anyhow::ensure!(
            self.cors_allowed_origins
                .iter()
                .all(|origin| origin == "*" || origin.parse::<axum::http::HeaderValue>().is_ok()),
            "cors_allowed_origins contains an invalid origin"
        );
        anyhow::ensure!(
            !self.require_auth
                || self
                    .admin_token
                    .as_deref()
                    .is_some_and(|token| !token.trim().is_empty()),
            "admin_token is required when require_auth is enabled"
        );
        anyhow::ensure!(
            !self
                .cors_allowed_origins
                .iter()
                .any(|origin| origin.trim() == "*")
                || self
                    .admin_token
                    .as_deref()
                    .is_some_and(|token| !token.trim().is_empty()),
            "admin_token is required when cors_allowed_origins contains '*'"
        );
        anyhow::ensure!(
            !self.history_file.trim().is_empty(),
            "history_file must not be empty"
        );
        self.validate_effective_bind_host(&self.host)?;
        Ok(())
    }

    pub fn validate_effective_bind_host(&self, host: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            is_loopback_host(host)
                || self
                    .admin_token
                    .as_deref()
                    .is_some_and(|token| !token.trim().is_empty()),
            "admin_token is required when binding EasyTex outside localhost"
        );
        Ok(())
    }
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host.trim(), "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

fn env_truthy(value: &str) -> bool {
    matches!(
        value.trim(),
        "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
    )
}

/// Project-specific settings structure parsed from local `EasyTex.toml` files.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    /// Relative filename of the root LaTeX document (defaults to `"main.tex"`).
    pub entrypoint: String,
    /// Optional command formatting template to override the default system formatter (`tex-fmt`).
    pub format_command: Option<String>,
}

impl Default for Config {
    /// Provides standard fallbacks for any new or non-configured project.
    fn default() -> Self {
        Self {
            entrypoint: "main.tex".into(),
            format_command: None,
        }
    }
}

/// Helper function to load project configurations asynchronously.
///
/// Searches for `EasyTex.toml` inside the designated directory and returns the deserialized settings.
/// If missing or structurally corrupt, falls back to `Config::default()`.
pub async fn read_cfg(root: &FsPath, name: &str) -> (Config, String) {
    let p = root.join(name).join("EasyTex.toml");
    let raw = fs_safety::read_text_limited(&p, MAX_CONFIG_SIZE)
        .await
        .unwrap_or_else(|_| "entrypoint = \"main.tex\"\n".into());
    match toml::from_str::<Config>(&raw) {
        Ok(cfg) if is_valid_entrypoint(&cfg.entrypoint) => (cfg, raw),
        Err(_) => (Config::default(), raw),
        _ => (Config::default(), raw),
    }
}
