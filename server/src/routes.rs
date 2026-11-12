// SPDX-License-Identifier: GPL-3.0-or-later

//! The HTTP API under `/v1`, as `docs/api.md` describes it.

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::watch;
use tower_http::cors::CorsLayer;

use crate::auth::Caller;
use crate::db::Db;
use crate::error::Error;
use crate::events::{Event, Hub};
use crate::reminders::{self, Reminder};

/// Request bodies are small JSON objects.
const BODY_LIMIT: usize = 64 * 1024;

/// What every handler shares.
#[derive(Clone)]
pub struct AppState {
    /// The database.
    pub db: Db,
    /// Live events.
    pub hub: Hub,
    /// Turns true when the server stops, which ends event streams.
    pub shutdown: watch::Receiver<bool>,
}

/// The API, with CORS for `origins` when there are any.
pub fn app(state: AppState, origins: &[HeaderValue]) -> Router {
    let mut router = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/token", get(token))
        .route("/v1/reminders", get(list).post(create))
        .route("/v1/reminders/{id}", get(read).patch(change).delete(delete))
        .route("/v1/reminders/{id}/done", post(done))
        .route("/v1/reminders/{id}/snooze", post(snooze))
        .route("/v1/events", get(events))
        .fallback(|| async { Error::not_found("no such endpoint") })
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .with_state(state);
    if !origins.is_empty() {
        router = router.layer(
            CorsLayer::new()
                .allow_origin(origins.to_vec())
                .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
                .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]),
        );
    }
    router
}

/// A JSON body, with an empty one read as `{}`.
fn body<T: DeserializeOwned>(bytes: Result<Bytes, BytesRejection>) -> Result<T, Error> {
    let bytes = bytes?;
    let bytes: &[u8] = if bytes.iter().all(u8::is_ascii_whitespace) {
        b"{}"
    } else {
        &bytes
    };
    serde_json::from_slice(bytes).map_err(|e| Error::invalid(e.to_string()))
}

fn id(raw: &str) -> Result<String, Error> {
    reminders::normal_id(raw).map_err(|_| Error::not_found("no such reminder"))
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

async fn token(Caller(name): Caller) -> Json<Value> {
    Json(json!({ "name": name }))
}

async fn list(
    _: Caller,
    State(state): State<AppState>,
    filter: Result<Query<reminders::Filter>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<reminders::Page>, Error> {
    let Query(filter) = filter?;
    let page = state
        .db
        .call(move |tx| reminders::list(tx, &filter))
        .await?;
    Ok(Json(page))
}

async fn create(
    Caller(by): Caller,
    State(state): State<AppState>,
    bytes: Result<Bytes, BytesRejection>,
) -> Result<impl IntoResponse, Error> {
    let input = body(bytes)?;
    let (reminder, made) = state
        .db
        .call(move |tx| reminders::create(tx, input, &by))
        .await?;
    if made {
        state.hub.send(Event::Changed(reminder.clone()));
    }
    let status = if made {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(reminder)))
}

async fn read(
    _: Caller,
    State(state): State<AppState>,
    Path(raw): Path<String>,
) -> Result<Json<Reminder>, Error> {
    let id = id(&raw)?;
    let reminder = state
        .db
        .call(move |tx| reminders::get(tx, &id))
        .await?
        .ok_or_else(|| Error::not_found("no such reminder"))?;
    Ok(Json(reminder))
}

/// Runs a write and announces its result.
async fn changed<F>(state: &AppState, f: F) -> Result<Json<Reminder>, Error>
where
    F: FnOnce(&rusqlite::Transaction<'_>) -> Result<Reminder, Error> + Send + 'static,
{
    let reminder = state.db.call(f).await?;
    state.hub.send(Event::Changed(reminder.clone()));
    Ok(Json(reminder))
}

async fn change(
    _: Caller,
    State(state): State<AppState>,
    Path(raw): Path<String>,
    bytes: Result<Bytes, BytesRejection>,
) -> Result<Json<Reminder>, Error> {
    let id = id(&raw)?;
    let changes = body(bytes)?;
    changed(&state, move |tx| reminders::change(tx, &id, changes)).await
}

async fn done(
    _: Caller,
    State(state): State<AppState>,
    Path(raw): Path<String>,
    bytes: Result<Bytes, BytesRejection>,
) -> Result<Json<Reminder>, Error> {
    let id = id(&raw)?;
    let input = body(bytes)?;
    changed(&state, move |tx| reminders::done(tx, &id, input)).await
}

async fn snooze(
    _: Caller,
    State(state): State<AppState>,
    Path(raw): Path<String>,
    bytes: Result<Bytes, BytesRejection>,
) -> Result<Json<Reminder>, Error> {
    let id = id(&raw)?;
    let input = body(bytes)?;
    changed(&state, move |tx| reminders::snooze(tx, &id, input)).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteQuery {
    version: Option<i64>,
}

async fn delete(
    _: Caller,
    State(state): State<AppState>,
    Path(raw): Path<String>,
    query: Result<Query<DeleteQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<StatusCode, Error> {
    let id = id(&raw)?;
    let Query(query) = query?;
    let gone = state
        .db
        .call(move |tx| reminders::delete(tx, &id, query.version))
        .await?;
    if let Some(r) = gone {
        state.hub.send(Event::Deleted {
            id: r.id,
            version: r.version,
        });
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn events(_: Caller, State(state): State<AppState>) -> impl IntoResponse {
    state.hub.stream(state.shutdown.clone())
}
