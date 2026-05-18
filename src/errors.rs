//! # Application Error Module
//!
//! This module defines the global, unified error model for the EasyTex system.
//! It contains `AppError`, an ergonomic, type-safe enum representing all possible runtime
//! failure scenarios (e.g. invalid client inputs, missing tool dependencies, read-only constraints,
//! and filesystem security boundary violations).
//!
//! `AppError` implements Axum's `IntoResponse` trait, enabling automatic serialization of error
//! payloads into structured JSON along with their corresponding HTTP status codes.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

/// Core error model representing failures within the LaTeX compilation server ecosystem.
///
/// Every variant maps to a precise HTTP status code and a descriptive error string sent to the
/// frontend UI.
#[derive(Debug)]
pub enum AppError {
    /// The client request is malformed, lacks required parameters, or contains structural syntax issues.
    BadRequest(String),
    /// Access was denied, typically due to path traversal detection or invalid authorization tokens.
    Forbidden(String),
    /// The requested project, run, file, or resource could not be found on disk.
    NotFound(String),
    /// Operation conflicts with the current server state (e.g., trying to recreate an existing project).
    Conflict(String),
    /// File upload, config save, or PDF serving exceeds configured payload thresholds.
    PayloadTooLarge(String),
    /// An operation that mutates state was attempted on a read-only server instance.
    ReadOnly,
    /// An optional tool (like `chktex` or `tex-fmt`) was requested but is not installed or available in PATH.
    DependencyMissing(&'static str),
    /// Catch-all for unexpected filesystem errors, subprocess spawns, or channel communication failures.
    Internal(String),
}

/// JSON payload structure returned by the server when an API endpoint encounters a failure.
#[derive(Serialize)]
struct ErrorResponse {
    /// Short machine-readable error token (e.g. `"payload_too_large"`).
    error: &'static str,
    /// Human-readable explanation of what went wrong.
    message: String,
}

impl AppError {
    /// Maps each error variant to its corresponding HTTP status code.
    fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Forbidden(_) | Self::ReadOnly => StatusCode::FORBIDDEN,
            Self::NotFound(_) | Self::DependencyMissing(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Returns a stable machine-readable identifier for frontend error handling logic.
    fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "bad_request",
            Self::Forbidden(_) => "forbidden",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::PayloadTooLarge(_) => "payload_too_large",
            Self::ReadOnly => "read_only",
            Self::DependencyMissing(_) => "dependency_missing",
            Self::Internal(_) => "internal_error",
        }
    }

    /// Resolves the final user-facing description of the error event.
    fn message(&self) -> String {
        match self {
            Self::BadRequest(message)
            | Self::Forbidden(message)
            | Self::NotFound(message)
            | Self::Conflict(message)
            | Self::PayloadTooLarge(message)
            | Self::Internal(message) => message.clone(),
            Self::ReadOnly => "Server is running in read-only mode".to_string(),
            Self::DependencyMissing(tool) => {
                format!("Optional dependency '{}' is not available in PATH", tool)
            }
        }
    }
}

impl IntoResponse for AppError {
    /// Standard Axum adapter turning an `AppError` into a fully formed HTTP JSON response.
    fn into_response(self) -> Response {
        (
            self.status(),
            Json(ErrorResponse {
                error: self.code(),
                message: self.message(),
            }),
        )
            .into_response()
    }
}

impl From<std::io::Error> for AppError {
    /// Ergonomic converter translating standard I/O disk failures into internal server errors.
    fn from(error: std::io::Error) -> Self {
        Self::Internal(error.to_string())
    }
}

/// Utility function generating a standard `"ok": true` response for successful void API calls.
pub fn ok_json() -> Response {
    Json(serde_json::json!({ "ok": true })).into_response()
}
