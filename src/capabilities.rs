//! # Capabilities Module
//!
//! This module provides dynamic detection of optional host system dependencies at startup.
//! EasyTex is designed to be highly resilient: if a tool (like `chktex` for linting or `gs` for PDF compression)
//! is missing from the system `PATH`, the server continues running but gracefully disables the corresponding
//! features in the UI and APIs.
//!
//! ## Monitored System Utilities
//!
//! * **`tex-fmt`**: Used for formatting LaTeX files directly from the web interface.
//! * **`chktex`**: Provides real-time LaTeX document linting and syntax checking.
//! * **`synctex`**: Translates coordinates between the compiled PDF viewer and the source editor.
//! * **`gs` (Ghostscript)**: Performs high-efficiency PDF size compression post-compilation.
//! * **`texloganalyser`**: Parses LaTeX `.log` compilation outputs to extract descriptive warnings.

use crate::utils::command_exists;
use serde::Serialize;
use ts_rs::TS;

/// Dynamic report of available commands and features on the host system.
///
/// This structure is populated at startup by querying the operating system's `PATH` for specific
/// executables. It is also exported as a TypeScript definition to let the SolidJS dashboard
/// automatically show/hide features and buttons according to server capabilities.
#[derive(Clone, Debug, Serialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub struct RuntimeCapabilities {
    /// True if `tex-fmt` is available for automatic document formatting.
    pub format: bool,
    /// True if `chktex` is available for real-time code quality linting.
    pub lint: bool,
    /// True if `synctex` is available for interactive editor-viewer navigation.
    pub synctex: bool,
    /// True if `gs` (Ghostscript) is available for compiling compressed preview PDFs.
    pub pdf_compression: bool,
    /// True if `texloganalyser` is available for extracting descriptive log analysis.
    pub log_analysis: bool,
    /// True if the server has been forced into read-only mode via configuration or environmental flags.
    pub read_only: bool,
}

impl RuntimeCapabilities {
    /// Auto-detects the presence of optional build and utility dependencies on the host system.
    ///
    /// Checks the PATH environment variable for required binaries using the `which` command-line utility.
    ///
    /// # Arguments
    ///
    /// * `read_only` - Force the read-only state, overriding other capabilities for mutative operations.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use easytex::capabilities::RuntimeCapabilities;
    ///
    /// let capabilities = RuntimeCapabilities::detect(false);
    /// println!("Is SyncTeX available? {}", capabilities.synctex);
    /// ```
    pub fn detect(read_only: bool) -> Self {
        Self {
            format: command_exists("tex-fmt"),
            lint: command_exists("chktex"),
            synctex: command_exists("synctex"),
            pdf_compression: command_exists("gs"),
            log_analysis: command_exists("texloganalyser"),
            read_only,
        }
    }
}

/// Metadata mapping an optional system binary to its specific purpose.
///
/// Used during initialization and diagnostics to check binary presence and display helpful warnings
/// if certain optional features are disabled.
pub struct OptionalTool {
    /// The exact command/binary name (e.g. `"chktex"`).
    pub command: &'static str,
    /// The name of the capability key associated with this tool (e.g. `"lint"`).
    pub capability: &'static str,
    /// User-friendly explanation of the feature that will be disabled if missing.
    pub description: &'static str,
}

/// The list of all optional host dependencies EasyTex can leverage to enhance compilation,
/// formatting, and linting.
pub const OPTIONAL_TOOLS: &[OptionalTool] = &[
    OptionalTool {
        command: "tex-fmt",
        capability: "format",
        description: "formatting from the UI",
    },
    OptionalTool {
        command: "chktex",
        capability: "lint",
        description: "linting from the UI",
    },
    OptionalTool {
        command: "synctex",
        capability: "synctex",
        description: "source/PDF navigation",
    },
    OptionalTool {
        command: "gs",
        capability: "pdf_compression",
        description: "PDF compression",
    },
    OptionalTool {
        command: "texloganalyser",
        capability: "log_analysis",
        description: "LaTeX log warning analysis",
    },
];
