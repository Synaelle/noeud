// SPDX-License-Identifier: GPL-3.0-or-later

//! The token check every endpoint but health goes through.

use axum::extract::FromRequestParts;
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;

use crate::error::Error;
use crate::routes::AppState;
use crate::tokens;

/// The name of the token that made the request.
pub struct Caller(pub String);

/// The token in an `Authorization: Bearer` header.
pub fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|v| v.trim().to_owned())
}

impl FromRequestParts<AppState> for Caller {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Error> {
        let token = bearer(&parts.headers).ok_or_else(Error::unauthorized)?;
        let now = crate::reminders::now().as_second();
        let name = state
            .db
            .call(move |tx| tokens::verify(tx, &token, now))
            .await?;
        if let Some(name) = name {
            return Ok(Self(name));
        }
        tracing::warn!(path = parts.uri.path(), "refused an unknown token");
        Err(Error::unauthorized())
    }
}
