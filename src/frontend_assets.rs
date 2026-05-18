//! # Embedded Frontend Assets Decompression Module
//!
//! This module manages the decompression and lookup of pre-compiled client-side build assets.
//!
//! Rather than compiling and loading individual flat files at compile-time (which can result in very
//! large executable size inflation and slow builds), EasyTex packs the entire SolidJS dashboard bundle
//! into a gzip-compressed binary archive at build time (typically via `build.rs` to `/frontend-dist.easytex.gz`).
//!
//! During startup, the server loads and decompresses the archive in-memory once, placing the decoded file
//! buffers inside a thread-safe `FrontendAssets` manager. This optimizes memory layout, drastically reduces
//! binary footprints, and guarantees rapid-fire web GUI asset delivery.

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use std::collections::HashMap;
use std::io::Read;

/// Pre-compiled client-side dashboard build bundle loaded from the compressed binary archive.
const FRONTEND_ARCHIVE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/frontend-dist.easytex.gz"));

/// Static distribution hash key used to force clients browser refresh on version updates.
pub const FRONTEND_DIST_HASH: &str = env!("EASYTEX_FRONTEND_DIST_HASH");

/// Thread-safe manager holding the decompressed in-memory SolidJS frontend asset files map.
#[derive(Clone)]
pub struct FrontendAssets {
    /// Internal hash map matching relative asset paths to decompressed byte arrays.
    files: HashMap<String, Vec<u8>>,
}

impl FrontendAssets {
    /// Decompresses the embedded gzip archive and parses the file structure into memory.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the archive is corrupt, truncated, or fails to decompress.
    pub fn load() -> Result<Self> {
        let mut decoder = GzDecoder::new(FRONTEND_ARCHIVE);
        let mut decoded = Vec::new();
        decoder
            .read_to_end(&mut decoded)
            .context("failed to decompress embedded frontend archive")?;
        let files = decode_archive(&decoded)?;
        Ok(Self { files })
    }

    /// Queries the in-memory files map and retrieves a pointer to the decompressed byte buffer.
    ///
    /// Returns `None` if the asset path does not exist.
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }

    /// Returns the total count of pre-compiled assets held inside the manager.
    pub fn len(&self) -> usize {
        self.files.len()
    }
}

/// Decodes the flat decompressed byte stream into a structured path-to-bytes HashMap.
///
/// Archive format:
/// For each file entry:
/// * 4-byte little-endian path string length
/// * 8-byte little-endian file contents byte length
/// * Path string bytes (UTF-8 encoded)
/// * File content bytes
///
/// # Errors
///
/// Returns `Err` if structural bounds, truncated entries, or invalid UTF-8 strings are detected.
fn decode_archive(bytes: &[u8]) -> Result<HashMap<String, Vec<u8>>> {
    let mut cursor = 0;
    let mut files = HashMap::new();

    while cursor < bytes.len() {
        let path_len = read_u32(bytes, &mut cursor)? as usize;
        let content_len = read_u64(bytes, &mut cursor)? as usize;

        anyhow::ensure!(
            cursor + path_len <= bytes.len(),
            "embedded frontend archive has a truncated path"
        );
        let path = std::str::from_utf8(&bytes[cursor..cursor + path_len])
            .context("embedded frontend archive contains an invalid UTF-8 path")?
            .to_string();
        cursor += path_len;

        anyhow::ensure!(
            cursor + content_len <= bytes.len(),
            "embedded frontend archive has truncated content for {path}"
        );
        let content = bytes[cursor..cursor + content_len].to_vec();
        cursor += content_len;

        files.insert(path, content);
    }

    anyhow::ensure!(
        files.contains_key("index.html"),
        "embedded frontend archive is missing index.html"
    );
    Ok(files)
}

/// Helper reading a 4-byte little-endian integer and advancing the cursor.
fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32> {
    anyhow::ensure!(
        *cursor + 4 <= bytes.len(),
        "embedded frontend archive is truncated"
    );
    let mut raw = [0_u8; 4];
    raw.copy_from_slice(&bytes[*cursor..*cursor + 4]);
    *cursor += 4;
    Ok(u32::from_le_bytes(raw))
}

/// Helper reading an 8-byte little-endian integer and advancing the cursor.
fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64> {
    anyhow::ensure!(
        *cursor + 8 <= bytes.len(),
        "embedded frontend archive is truncated"
    );
    let mut raw = [0_u8; 8];
    raw.copy_from_slice(&bytes[*cursor..*cursor + 8]);
    *cursor += 8;
    Ok(u64::from_le_bytes(raw))
}

#[cfg(test)]
mod tests {
    use super::FrontendAssets;

    #[test]
    fn embedded_frontend_archive_loads_index() {
        let assets = FrontendAssets::load().unwrap();
        assert!(assets.get("index.html").is_some());
        assert!(assets.len() > 0);
    }
}
