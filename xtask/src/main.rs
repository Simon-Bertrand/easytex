use anyhow::{bail, Context, Result};
use std::process::Command;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("generate-types") => generate_types(),
        Some(command) => bail!("unknown xtask command: {command}"),
        None => {
            eprintln!("usage: cargo run -p xtask -- generate-types");
            Ok(())
        }
    }
}

fn generate_types() -> Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = Command::new(cargo)
        .args(["test", "-p", "easytex", "export_bindings"])
        .env("EASYTEX_GENERATE_TYPES", "1")
        .status()
        .context("failed to run cargo test export_bindings")?;

    if !status.success() {
        bail!("TypeScript binding generation failed");
    }

    Ok(())
}
