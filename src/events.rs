//! # Server Event Streams (SSE) Module
//!
//! This module encapsulates the message-passing schemas for the Server-Sent Events (SSE) stream.
//! When a client opens an SSE connection, the backend broadcasts real-time compilation progress,
//! compiler stdout/stderr logs, linter diagnostics, word-count reports, and document reload signals.
//!
//! These events are formatted to JSON, encoded in the SSE-standard protocol (`data: <json>\n\n`),
//! and seamlessly mapped to TypeScript bindings for frontend consumer components.

use serde::Serialize;
use ts_rs::TS;

/// A parsed LaTeX linter or compiler diagnostic warning/error.
///
/// Extracted dynamically from standard error or output streams (e.g. from Tectonic or latexmk).
#[derive(Clone, Debug, Serialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub struct DiagnosticEvent {
    /// Severity level of the issue (typically `"warn"` or `"err"`).
    pub severity: String,
    /// Absolute or relative path of the source LaTeX file where the error occurred.
    pub file: Option<String>,
    /// Line number (1-based index) of the diagnostic target.
    pub line: Option<u32>,
    /// Column number (1-based index) of the diagnostic target.
    pub column: Option<u32>,
    /// Sanitized, human-readable description of the warning or error.
    pub message: String,
    /// The exact raw log line produced by the compilation sub-process.
    pub raw: String,
}

/// The set of message payloads that can be pushed over the SSE channel.
///
/// Designed with serde-tagging (`"type": "..."`) to match idiomatic TypeScript unions on the frontend.
#[derive(Serialize, TS)]
#[serde(tag = "type", content = "data")]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub enum ServerEvent {
    /// Tells the client that the server's build state has changed (e.g. `"Idle"`, `"Manual Build"`, `"Error"`).
    Status(String),
    /// Pushes a console log line from the compiler to the frontend's build console drawer.
    Log {
        /// Severity level of the log line (e.g., `"ok"`, `"warn"`, `"err"`, `"dim"`).
        lvl: String,
        /// Raw message string.
        msg: String,
    },
    /// Dispatches a structured compiler diagnostic indicating a warning or error.
    Diagnostic(DiagnosticEvent),
    /// Warns the UI that a project file has changed on disk, prompting refresh states.
    FileChanged {
        /// Relative path of the modified file.
        path: String,
    },
    /// Signals the PDF viewer component to reload the compiled PDF file (typically sends `"reload"`).
    Pdf(String),
    /// Supplies key telemetry about a completed build.
    Stats {
        /// Format string of compilation duration (e.g. `"1.2s"`).
        time: String,
        /// Size representation of the compiled PDF on disk (e.g. `"450 KB"`).
        size: String,
        /// Deep word count calculated across all recursive LaTeX inputs.
        words: u32,
    },
}

impl ServerEvent {
    /// Encodes the `ServerEvent` enum into a valid JSON string.
    ///
    /// # Errors
    ///
    /// If JSON serialization fails, it falls back to producing an emergency `"err"` log event string.
    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            r#"{"type":"log","data":{"lvl":"err","msg":"Failed to encode server event"}}"#
                .to_string()
        })
    }
}

/// Utility constructor to encode an active server status update into JSON.
pub fn status(status: &'static str) -> String {
    ServerEvent::Status(status.to_string()).encode()
}

/// Utility constructor to encode a single line of standard logs into JSON.
pub fn log(lvl: impl Into<String>, msg: impl Into<String>) -> String {
    ServerEvent::Log {
        lvl: lvl.into(),
        msg: msg.into(),
    }
    .encode()
}

/// Utility constructor to encode a compiler/linter diagnostic event into JSON.
pub fn diagnostic(value: DiagnosticEvent) -> String {
    ServerEvent::Diagnostic(value).encode()
}

/// Utility constructor to encode a disk file modification notice into JSON.
pub fn file_changed(path: impl Into<String>) -> String {
    ServerEvent::FileChanged { path: path.into() }.encode()
}

/// Utility constructor to encode a PDF reload trigger event.
pub fn pdf_reload() -> String {
    ServerEvent::Pdf("reload".to_string()).encode()
}

/// Utility constructor to encode compiled LaTeX size, time, and word telemetry.
pub fn stats(time: impl Into<String>, size: impl Into<String>, words: u32) -> String {
    ServerEvent::Stats {
        time: time.into(),
        size: size.into(),
        words,
    }
    .encode()
}
