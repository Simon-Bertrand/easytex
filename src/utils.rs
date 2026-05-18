//! # Utility and Path Sanitization Module
//!
//! This module contains helper functions and safety check routines for validating paths,
//! file extensions, project names, and verifying the existence of system commands.
//!
//! ## Security Boundaries
//!
//! A primary concern of this server is preventing directory traversal attacks.
//! This module implements robust checks (`safe_path` and `safe_project_file`) to ensure
//! all filesystem operations are strictly sandboxed within the configured project and root directories.

use std::path::{Path as FsPath, PathBuf};
use std::process::{Command, Stdio};

/// Maximum allowed length for a project name to prevent filesystem overflow issues.
pub const MAX_PROJECT_NAME_LEN: usize = 256;

/// Maximum allowed byte size for a project's `EasyTex.toml` file to avoid high memory usage.
pub const MAX_CONFIG_SIZE: usize = 10_000;

/// White-listed list of document extensions that can be opened and edited through the UI.
const EDITABLE_EXTENSIONS: &[&str] = &[
    "tex", "toml", "bib", "sty", "cls", "tikz", "txt", "md", "svg",
];

/// Validates whether a project name contains only alphanumeric characters, dashes, or underscores,
/// and satisfies size constraints.
///
/// # Examples
///
/// ```
/// use easytex::utils::is_valid_project_name;
///
/// assert!(is_valid_project_name("thesis_2026"));
/// assert!(!is_valid_project_name("../etc/passwd"));
/// ```
pub fn is_valid_project_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_PROJECT_NAME_LEN {
        return false;
    }
    name.chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

/// Validates that a LaTeX entrypoint file matches basic formatting requirements.
///
/// The file must end with `.tex`, contain no double dots (`..`), start with a valid character,
/// and consist only of alphanumeric symbols, dots, hyphens, or underscores.
///
/// # Examples
///
/// ```
/// use easytex::utils::is_valid_entrypoint;
///
/// assert!(is_valid_entrypoint("main.tex"));
/// assert!(!is_valid_entrypoint("main.txt"));
/// ```
pub fn is_valid_entrypoint(entrypoint: &str) -> bool {
    if entrypoint.is_empty() || entrypoint.len() > 256 || entrypoint.starts_with('.') {
        return false;
    }
    entrypoint
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
        && !entrypoint.contains("..")
        && entrypoint.ends_with(".tex")
}

/// Checks if a file path is safe to be edited inside the project workspace.
///
/// Prevents paths that start with `/`, `.` or contain backslashes or invalid extensions
/// to block parent directory traversal attacks.
pub fn is_editable_project_file(path: &str) -> bool {
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('.')
        || path.contains(':')
        || path.contains('\\')
        || path.starts_with("//")
        || path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == ".." || p.starts_with('.'))
    {
        return false;
    }

    let Some(ext) = FsPath::new(path).extension().and_then(|s| s.to_str()) else {
        return false;
    };
    EDITABLE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
}

/// Resolves and guarantees that a project file resides strictly within the project directory.
///
/// First checks `is_editable_project_file`, then canonicalizes parent paths and asserts that the
/// final absolute path starts with the project directory's root. Returns `None` on traversal attempts.
pub fn safe_project_file(project_dir: &FsPath, relative_path: &str) -> Option<PathBuf> {
    if !is_editable_project_file(relative_path) {
        tracing::warn!("Rejected unsafe project file path: {}", relative_path);
        return None;
    }

    let root = project_dir.canonicalize().ok()?;
    let candidate = root.join(relative_path);
    let parent = candidate.parent()?.canonicalize().ok()?;
    if parent.starts_with(&root) {
        Some(candidate)
    } else {
        tracing::warn!("Rejected path outside project: {}", relative_path);
        None
    }
}

/// Sanity-checks a project name and returns its absolute path within the workspace root.
///
/// Prevents any traversal outside the system's root dir.
pub fn safe_path(root: &FsPath, name: &str) -> Option<PathBuf> {
    if !is_valid_project_name(name) {
        tracing::warn!("Rejected invalid project name: {}", name);
        return None;
    }
    let root_canon = match root.canonicalize() {
        Ok(r) => r,
        Err(_) => {
            tracing::error!("Failed to canonicalize root: {}", root.display());
            return None;
        }
    };
    let candidate = root_canon.join(name);
    match candidate.canonicalize() {
        Ok(path) => {
            if path.starts_with(&root_canon) {
                Some(path)
            } else {
                tracing::warn!(
                    "Path traversal attempt: {} outside {}",
                    path.display(),
                    root_canon.display()
                );
                None
            }
        }
        Err(_) => {
            if let Some(parent) = candidate.parent() {
                match parent.canonicalize() {
                    Ok(parent_canon) => {
                        if parent_canon == root_canon || parent_canon.starts_with(&root_canon) {
                            Some(candidate)
                        } else {
                            tracing::warn!(
                                "Parent path traversal: {} outside {}",
                                parent_canon.display(),
                                root_canon.display()
                            );
                            None
                        }
                    }
                    Err(_) => {
                        tracing::warn!("Failed to canonicalize parent of {}", candidate.display());
                        None
                    }
                }
            } else {
                None
            }
        }
    }
}

/// Generates a pseudo-random hex string of specified length using system nanosecond timestamp.
pub fn rand_hex_string(len: usize) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let s = format!("{:x}", nanos);
    if s.len() > len {
        s[s.len() - len..].to_string()
    } else {
        s
    }
}

/// Checks whether a specific system command exists in the operating system's PATH.
///
/// Uses the `which` command-line tool internally to verify process execution viability.
pub fn command_exists(command: &str) -> bool {
    Command::new("which")
        .arg(command)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
