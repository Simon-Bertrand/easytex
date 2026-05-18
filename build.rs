use flate2::{write::GzEncoder, Compression};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

fn main() -> anyhow::Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=frontend/dist");

    let dist_dir = Path::new("frontend/dist");
    if !dist_dir.join("index.html").is_file() {
        if is_generating_types() {
            let hash = FNV_OFFSET;
            let archive_path = build_frontend_archive_from_bytes(vec![(
                "index.html".to_string(),
                b"<!doctype html><title>EasyTex type generation placeholder</title>".to_vec(),
            )])?;
            println!("cargo:rustc-env=EASYTEX_FRONTEND_DIST_HASH={hash:016x}");
            println!(
                "cargo:info=Using placeholder frontend archive for TypeScript binding generation {}",
                archive_path.display()
            );
            return Ok(());
        }

        anyhow::bail!(
            "frontend/dist is missing or incomplete. Build the UI first, for example: cd frontend && bun run build"
        );
    }

    let hash = frontend_dist_hash(dist_dir)?;
    let archive_path = build_frontend_archive(dist_dir)?;
    println!("cargo:rustc-env=EASYTEX_FRONTEND_DIST_HASH={hash:016x}");
    println!("cargo:info=Using prebuilt frontend/dist hash {hash:016x}");
    println!(
        "cargo:info=Embedded compressed frontend archive {}",
        archive_path.display()
    );

    Ok(())
}

fn is_generating_types() -> bool {
    std::env::var("EASYTEX_GENERATE_TYPES").is_ok_and(|value| value == "1" || value == "true")
}

fn build_frontend_archive(dist_dir: &Path) -> anyhow::Result<PathBuf> {
    let mut files = Vec::new();
    collect_files(dist_dir, dist_dir, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let files = files
        .into_iter()
        .map(|(relative, absolute)| fs::read(&absolute).map(|content| (relative, content)))
        .collect::<Result<Vec<_>, _>>()?;

    build_frontend_archive_from_bytes(files)
}

fn build_frontend_archive_from_bytes(files: Vec<(String, Vec<u8>)>) -> anyhow::Result<PathBuf> {
    let mut archive = Vec::new();
    for (relative, content) in files {
        let path_bytes = relative.as_bytes();
        anyhow::ensure!(
            path_bytes.len() <= u32::MAX as usize,
            "frontend asset path is too long: {relative}"
        );
        archive.write_all(&(path_bytes.len() as u32).to_le_bytes())?;
        archive.write_all(&(content.len() as u64).to_le_bytes())?;
        archive.write_all(path_bytes)?;
        archive.write_all(&content)?;
    }

    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(&archive)?;
    let compressed = encoder.finish()?;
    let out_dir = std::env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("OUT_DIR is not set"))?;
    let archive_path = out_dir.join("frontend-dist.easytex.gz");
    fs::write(&archive_path, compressed)?;
    Ok(archive_path)
}

fn frontend_dist_hash(dist_dir: &Path) -> anyhow::Result<u64> {
    let mut files = Vec::new();
    collect_files(dist_dir, dist_dir, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hash = FNV_OFFSET;
    for (relative, absolute) in files {
        println!("cargo:rerun-if-changed={}", absolute.display());
        hash_bytes(&mut hash, relative.as_bytes());
        hash_bytes(&mut hash, b"\0");
        let bytes = fs::read(&absolute)?;
        hash_bytes(&mut hash, &bytes);
        hash_bytes(&mut hash, b"\0");
    }

    Ok(hash)
}

fn collect_files(
    root: &Path,
    dir: &Path,
    files: &mut Vec<(String, PathBuf)>,
) -> anyhow::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, files)?;
        } else if path.is_file() {
            let relative = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            files.push((relative, path));
        }
    }
    Ok(())
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}
