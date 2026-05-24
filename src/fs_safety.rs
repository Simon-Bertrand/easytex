//! # Sandboxed Filesystem Operations Module
//!
//! This module implements high-rigor, safe filesystem operations designed to operate inside a sandbox.
//! By wrapping all reads, writes, and list operations, it prevents directory traversal exploits,
//! limits maximum size memory consumption (protecting against Denial-of-Service / zip-bomb styles),
//! and blocks operations on unsafe file types like symbolic links.

use std::path::{Path as FsPath, PathBuf};

use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
};

use crate::{
    errors::AppError,
    utils::{is_editable_project_file, rand_hex_string, safe_project_file},
};

/// Represents a validated project file with verified safe absolute and relative paths.
pub struct ProjectFile {
    /// Relative path from the project directory (e.g. `"chapters/intro.tex"`).
    pub relative_path: String,
    /// Absolute canonical path on the host filesystem.
    pub absolute_path: PathBuf,
}

/// Contains a list of found project files along with completeness status indicators.
pub struct ProjectFileList {
    /// Vector of relative filenames matching safe extension rules.
    pub files: Vec<String>,
    /// Set to `false` if the scanning limits were reached and listing was truncated.
    pub complete: bool,
}

/// Validates a relative file request and returns a secure `ProjectFile` handle.
///
/// Refuses any requests containing traversal paths or non-editable extensions.
///
/// # Errors
///
/// Returns `AppError::Forbidden` if validation checks fail or the file points outside the project root.
pub fn resolve_project_file(
    project_dir: &FsPath,
    relative_path: impl Into<String>,
) -> Result<ProjectFile, AppError> {
    let relative_path = relative_path.into();
    if !is_editable_project_file(&relative_path) {
        return Err(AppError::Forbidden("Invalid project file path".into()));
    }
    let absolute_path = safe_project_file(project_dir, &relative_path)
        .ok_or_else(|| AppError::Forbidden("Project file is outside the project".into()))?;
    Ok(ProjectFile {
        relative_path,
        absolute_path,
    })
}

/// Reads the textual content of a sandboxed file, enforcing size limits and blocking symlinks.
///
/// # Errors
///
/// * Returns `AppError::Forbidden` if target is a symbolic link.
/// * Returns `AppError::PayloadTooLarge` if file size exceeds `max_bytes`.
/// * Returns `AppError::NotFound` if the target file does not exist on disk.
pub async fn read_text_limited(path: &FsPath, max_bytes: usize) -> Result<String, AppError> {
    if fs::symlink_metadata(path)
        .await
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(AppError::Forbidden("Symlinks are not readable".into()));
    }

    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        options.custom_flags(libc::O_NOFOLLOW);
    }

    let file = options.open(path).await.map_err(|error| {
        #[cfg(unix)]
        if error.raw_os_error() == Some(libc::ELOOP) {
            return AppError::Forbidden("Symlinks are not readable".into());
        }
        match error.kind() {
            std::io::ErrorKind::NotFound => AppError::NotFound("File not found".into()),
            _ => error.into(),
        }
    })?;

    let metadata = file.metadata().await?;
    if metadata.len() as usize > max_bytes {
        return Err(AppError::PayloadTooLarge(format!(
            "File too large ({} bytes, max {} bytes)",
            metadata.len(),
            max_bytes
        )));
    }

    let mut content = String::new();
    file.take((max_bytes as u64).saturating_add(1))
        .read_to_string(&mut content)
        .await?;
    if content.len() > max_bytes {
        return Err(AppError::PayloadTooLarge(format!(
            "File too large (max {} bytes)",
            max_bytes
        )));
    }
    Ok(content)
}

/// Writes textual content to a sandboxed file securely.
///
/// Enforces length limits, blocks symlink writes, and automatically creates parent directories.
///
/// # Errors
///
/// * Returns `AppError::PayloadTooLarge` if write buffer size exceeds `max_bytes`.
/// * Returns `AppError::Forbidden` if target is a symbolic link.
pub async fn write_text_limited(
    path: &FsPath,
    content: &str,
    max_bytes: usize,
) -> Result<(), AppError> {
    if content.len() > max_bytes {
        return Err(AppError::PayloadTooLarge(format!(
            "File too large (max {} bytes)",
            max_bytes
        )));
    }

    if fs::symlink_metadata(path)
        .await
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(AppError::Forbidden("Symlinks are not writable".into()));
    }

    let Some(parent) = path.parent() else {
        return Err(AppError::BadRequest("Invalid file path".into()));
    };
    fs::create_dir_all(parent).await?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::BadRequest("Invalid file path".into()))?;
    let tmp_path = parent.join(format!(".{}.{}.tmp", file_name, rand_hex_string(12)));

    let write_result = async {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        let mut tmp = options.open(&tmp_path).await?;
        tmp.write_all(content.as_bytes()).await?;
        tmp.sync_data().await?;
        fs::rename(&tmp_path, path).await?;
        Ok::<(), std::io::Error>(())
    }
    .await;

    if let Err(error) = write_result {
        let _ = fs::remove_file(&tmp_path).await;
        return Err(error.into());
    }
    Ok(())
}

/// Recursively scans and lists all editable files within a project directory up to a fixed threshold.
///
/// Automatically excludes hidden directories, system files, symlinks, and the `"build"` directory.
///
/// # Errors
///
/// Returns filesystem errors as `AppError::Internal`.
pub async fn list_project_files(
    project_dir: PathBuf,
    max_files: usize,
) -> Result<ProjectFileList, AppError> {
    /// Internal recursive walk engine.
    async fn walk(dir: PathBuf, base: PathBuf, list: &mut Vec<String>, limit: usize) -> bool {
        if list.len() >= limit {
            return false;
        }
        if let Ok(mut rd) = fs::read_dir(&dir).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                if list.len() >= limit {
                    return false;
                }
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if name.starts_with('.') || name == "build" {
                    continue;
                }
                let Ok(metadata) = fs::symlink_metadata(&path).await else {
                    continue;
                };
                if metadata.file_type().is_symlink() {
                    continue;
                }
                if metadata.is_dir() {
                    if !Box::pin(walk(path, base.clone(), list, limit)).await {
                        return false;
                    }
                } else if let Ok(rel) = path.strip_prefix(&base) {
                    let relative = rel.to_string_lossy().to_string();
                    if is_editable_project_file(&relative) {
                        list.push(relative);
                    }
                }
            }
        }
        true
    }

    let mut files = Vec::new();
    let complete = walk(project_dir.clone(), project_dir, &mut files, max_files).await;
    files.sort();
    Ok(ProjectFileList { files, complete })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_project(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "easytex-{}-{}-{}",
            name,
            std::process::id(),
            suffix
        ))
    }

    #[tokio::test]
    async fn read_text_limited_rejects_large_files() {
        let root = temp_project("large-file");
        fs::create_dir_all(&root).await.unwrap();
        let file = root.join("main.tex");
        fs::write(&file, "abcdef").await.unwrap();

        let err = read_text_limited(&file, 3).await.unwrap_err();
        assert!(matches!(err, AppError::PayloadTooLarge(_)));

        let _ = fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn list_project_files_reports_truncation() {
        let root = temp_project("list-limit");
        fs::create_dir_all(&root).await.unwrap();
        fs::write(root.join("a.tex"), "").await.unwrap();
        fs::write(root.join("b.tex"), "").await.unwrap();

        let listed = list_project_files(root.clone(), 1).await.unwrap();
        assert_eq!(listed.files.len(), 1);
        assert!(!listed.complete);

        let _ = fs::remove_dir_all(&root).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_text_limited_rejects_symlinks() {
        let root = temp_project("symlink");
        fs::create_dir_all(&root).await.unwrap();
        let target = root.join("target.tex");
        let link = root.join("link.tex");
        fs::write(&target, "hello").await.unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let err = read_text_limited(&link, 1024).await.unwrap_err();
        assert!(matches!(err, AppError::Forbidden(_)));

        let _ = fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn list_project_files_only_returns_editable_files() {
        let root = temp_project("list-editable");
        fs::create_dir_all(root.join("build")).await.unwrap();
        fs::write(root.join("main.tex"), "").await.unwrap();
        fs::write(root.join("secret.bin"), "").await.unwrap();
        fs::write(root.join("build").join("generated.tex"), "")
            .await
            .unwrap();

        let listed = list_project_files(root.clone(), 10).await.unwrap();
        assert_eq!(listed.files, vec!["main.tex"]);
        assert!(listed.complete);

        let _ = fs::remove_dir_all(&root).await;
    }
}
