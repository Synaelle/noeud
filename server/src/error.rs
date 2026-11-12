// SPDX-License-Identifier: GPL-3.0-or-later

//! Failed requests as `{ "error": code, "message": text }` with the right
//! status. Internal details go to the log, never to the response.

use axum::Json;
use axum::extract::rejection::{BytesRejection, QueryRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::db::DbError;
use crate::reminders::Reminder;

/// An error a handler returns.
#[derive(Debug)]
pub struct Error {
    status: StatusCode,
    code: &'static str,
    message: String,
    reminder: Option<Box<Reminder>>,
}

impl Error {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            reminder: None,
        }
    }

    /// A request the endpoint cannot use, `message` says why.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    /// No token, or one the server does not know.
    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a valid token is needed",
        )
    }

    /// No such reminder or endpoint.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    /// The client's version is not the current one, which comes along.
    pub fn conflict(current: Reminder) -> Self {
        Self {
            reminder: Some(Box::new(current)),
            ..Self::new(
                StatusCode::CONFLICT,
                "conflict",
                "the reminder was changed in the meantime",
            )
        }
    }

    /// Something broke, the details go to the log.
    pub fn internal(detail: &str) -> Self {
        tracing::error!("{detail}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal error",
        )
    }
}

impl From<DbError> for Error {
    fn from(err: DbError) -> Self {
        Self::internal(&err.to_string())
    }
}

impl From<BytesRejection> for Error {
    fn from(rejection: BytesRejection) -> Self {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            return Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "too_large",
                "bodies are limited to 64 KB",
            );
        }
        Self::invalid(rejection.body_text())
    }
}

impl From<QueryRejection> for Error {
    fn from(rejection: QueryRejection) -> Self {
        Self::invalid(rejection.body_text())
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let mut body = json!({ "error": self.code, "message": self.message });
        if let Some(reminder) = self.reminder {
            body["reminder"] = serde_json::to_value(reminder).unwrap_or(Value::Null);
        }
        (self.status, Json(body)).into_response()
    }
}
