// SPDX-License-Identifier: GPL-3.0-or-later

//! The API end to end, through the router on a database in memory.

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_stream::StreamExt;
use tower::ServiceExt;

use crate::db::Db;
use crate::events::Hub;
use crate::routes::{self, AppState};
use crate::tokens;

struct Server {
    app: Router,
    token: String,
    db: Db,
    _stop: watch::Sender<bool>,
}

async fn server() -> Server {
    let db = Db::in_memory();
    let token = db
        .call(|tx| tokens::create(tx, "tests", 0))
        .await
        .unwrap()
        .unwrap();
    let (stop, shutdown) = watch::channel(false);
    let hub = Hub::default();
    tokio::spawn(hub.clone().run_scheduler(db.clone(), shutdown.clone()));
    let app = routes::app(
        AppState {
            db: db.clone(),
            hub,
            shutdown,
        },
        &[],
    );
    Server {
        app,
        token,
        db,
        _stop: stop,
    }
}

impl Server {
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.send_as(Some(&self.token.clone()), method, path, body)
            .await
    }

    async fn send_as(
        &self,
        token: Option<&str>,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(path);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
        let res = self
            .app
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, value)
    }

    async fn create(&self, body: Value) -> Value {
        let (status, r) = self.send("POST", "/v1/reminders", Some(body)).await;
        assert_eq!(status, StatusCode::CREATED, "{r}");
        r
    }
}

fn in_minutes(m: i64) -> String {
    Timestamp::now()
        .checked_add(SignedDuration::from_mins(m))
        .unwrap()
        .round(jiff::Unit::Second)
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn health_needs_no_token() {
    let s = server().await;
    let (status, body) = s.send_as(None, "GET", "/v1/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn tokens_are_checked() {
    let s = server().await;
    let (status, body) = s.send_as(None, "GET", "/v1/reminders", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "unauthorized");
    let (status, _) = s
        .send_as(Some("noeud_nothing"), "GET", "/v1/reminders", None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = s.send("GET", "/v1/token", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "tests");

    s.db.call(|tx| tokens::revoke(tx, "tests")).await.unwrap();
    let (status, _) = s.send("GET", "/v1/token", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_regenerated_token_replaces_the_old_one() {
    let s = server().await;
    let r = s
        .create(json!({ "title": "before", "due_at": "2026-11-13T09:00:00Z" }))
        .await;
    let fresh =
        s.db.call(|tx| tokens::regenerate(tx, "tests", 1))
            .await
            .unwrap()
            .expect("the token exists");
    assert_ne!(fresh, s.token);

    let (status, _) = s.send("GET", "/v1/token", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the old token is refused");
    let (status, body) = s
        .send_as(
            Some(&fresh),
            "GET",
            &format!("/v1/reminders/{}", r["id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the new one reaches the same reminders"
    );
    assert_eq!(body["created_by"], "tests");

    let none =
        s.db.call(|tx| tokens::regenerate(tx, "nobody", 1))
            .await
            .unwrap();
    assert_eq!(none, None);
}

#[tokio::test]
async fn create_and_read() {
    let s = server().await;
    let r = s
        .create(json!({
            "title": "  Call the dentist ",
            "due_at": "2026-11-13T10:00:00+01:00",
            "time_zone": "Europe/Paris",
        }))
        .await;
    assert_eq!(r["title"], "Call the dentist");
    assert_eq!(r["due_at"], "2026-11-13T09:00:00Z");
    assert_eq!(r["version"], 1);
    assert_eq!(r["created_by"], "tests");
    assert_eq!(r["deleted"], false);
    assert_eq!(r["repeat"], Value::Null);

    let id = r["id"].as_str().unwrap();
    let (status, got) = s.send("GET", &format!("/v1/reminders/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, r);
    let (status, _) = s
        .send(
            "GET",
            "/v1/reminders/0193a000-0000-7000-8000-000000000000",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = s.send("GET", "/v1/reminders/nope", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_with_an_id_is_idempotent() {
    let s = server().await;
    let body = json!({
        "id": "0193A4E8-8C2F-7CC1-A1B2-3C4D5E6F7A8B",
        "title": "Water the plants",
        "due_at": "2026-11-13T09:00:00Z",
    });
    let first = s.create(body.clone()).await;
    assert_eq!(first["id"], "0193a4e8-8c2f-7cc1-a1b2-3c4d5e6f7a8b");
    let (status, again) = s.send("POST", "/v1/reminders", Some(body)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again, first);
}

#[tokio::test]
async fn bad_input_is_refused() {
    let s = server().await;
    for body in [
        json!({ "title": "", "due_at": "2026-11-13T09:00:00Z" }),
        json!({ "title": "x".repeat(501), "due_at": "2026-11-13T09:00:00Z" }),
        json!({ "title": "a", "due_at": "tomorrow" }),
        json!({ "title": "a", "due_at": "2026-11-13T09:00:00Z", "time_zone": "Mars/Base" }),
        json!({ "title": "a", "due_at": "2026-11-13T09:00:00Z", "dueAt": 1 }),
        json!({ "title": "a", "due_at": "2026-11-13T09:00:00Z", "repeat": { "every": "hour" } }),
        json!({ "title": "a", "due_at": "2026-11-13T09:00:00Z",
                "repeat": { "every": "day", "interval": 0 } }),
        json!({ "title": "a", "due_at": "2026-11-13T09:00:00Z", "id": "not-a-uuid" }),
    ] {
        let (status, err) = s.send("POST", "/v1/reminders", Some(body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(err["error"], "invalid_request");
    }
    let (status, _) = s.send("GET", "/v1/reminders?limit=0", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = s.send("GET", "/v1/reminders?colour=red", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn large_bodies_are_refused() {
    let s = server().await;
    let body =
        json!({ "title": "a", "due_at": "2026-11-13T09:00:00Z", "notes": "x".repeat(70_000) });
    let (status, err) = s.send("POST", "/v1/reminders", Some(body)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(err["error"], "too_large");
}

#[tokio::test]
async fn list_soonest_first_with_filters() {
    let s = server().await;
    let later = s
        .create(json!({ "title": "later", "due_at": "2026-12-01T09:00:00Z" }))
        .await;
    let soon = s
        .create(json!({ "title": "soon", "due_at": "2026-11-01T09:00:00Z" }))
        .await;
    let id = soon["id"].as_str().unwrap();
    s.send("POST", &format!("/v1/reminders/{id}/done"), None)
        .await;

    let (_, page) = s.send("GET", "/v1/reminders", None).await;
    let titles: Vec<_> = page["reminders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].clone())
        .collect();
    assert_eq!(titles, ["soon", "later"]);
    assert_eq!(page["has_more"], false);

    let (_, page) = s.send("GET", "/v1/reminders?done=false", None).await;
    assert_eq!(page["reminders"][0]["id"], later["id"]);
    assert_eq!(page["reminders"].as_array().unwrap().len(), 1);

    let (_, page) = s
        .send("GET", "/v1/reminders?due_before=2026-11-15T00:00:00Z", None)
        .await;
    assert_eq!(page["reminders"].as_array().unwrap().len(), 1);
    assert_eq!(page["reminders"][0]["title"], "soon");
}

#[tokio::test]
async fn change_with_versions() {
    let s = server().await;
    let r = s
        .create(json!({ "title": "a", "notes": "n", "due_at": "2026-11-13T09:00:00Z" }))
        .await;
    let path = format!("/v1/reminders/{}", r["id"].as_str().unwrap());

    let (status, r2) = s
        .send(
            "PATCH",
            &path,
            Some(json!({ "version": 1, "title": "b", "notes": null })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(r2["title"], "b");
    assert_eq!(r2["notes"], Value::Null);
    assert_eq!(r2["version"], 2);

    let (status, err) = s
        .send("PATCH", &path, Some(json!({ "version": 1, "title": "c" })))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(err["error"], "conflict");
    assert_eq!(err["reminder"], r2);

    let (status, _) = s
        .send("PATCH", &path, Some(json!({ "version": 2, "title": null })))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = s.send("PATCH", &path, Some(json!({ "title": "d" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn moving_due_at_clears_the_snooze_and_moves_the_repeat() {
    let s = server().await;
    let r = s
        .create(json!({
            "title": "standup",
            "due_at": "2026-11-13T09:00:00Z",
            "repeat": { "every": "week" },
        }))
        .await;
    assert_eq!(
        r["repeat"],
        json!({ "every": "week", "interval": 1, "start": "2026-11-13T09:00:00Z" })
    );
    let path = format!("/v1/reminders/{}", r["id"].as_str().unwrap());
    let (_, r) = s
        .send(
            "POST",
            &format!("{path}/snooze"),
            Some(json!({ "minutes": 10 })),
        )
        .await;
    assert!(r["snoozed_until"].is_string());
    let (_, r) = s
        .send(
            "PATCH",
            &path,
            Some(json!({ "version": 2, "due_at": "2026-11-14T10:00:00Z" })),
        )
        .await;
    assert_eq!(r["snoozed_until"], Value::Null);
    assert_eq!(r["repeat"]["start"], "2026-11-14T10:00:00Z");
}

#[tokio::test]
async fn done_and_repeats() {
    let s = server().await;
    let once = s
        .create(json!({ "title": "once", "due_at": "2026-01-01T09:00:00Z" }))
        .await;
    let (_, once) = s
        .send(
            "POST",
            &format!("/v1/reminders/{}/done", once["id"].as_str().unwrap()),
            None,
        )
        .await;
    assert!(once["done_at"].is_string());

    let daily = s
        .create(json!({
            "title": "pills",
            "due_at": "2026-01-01T08:00:00Z",
            "time_zone": "Europe/Paris",
            "repeat": { "every": "day", "interval": 1 },
        }))
        .await;
    let path = format!("/v1/reminders/{}/done", daily["id"].as_str().unwrap());
    let (_, moved) = s.send("POST", &path, Some(json!({ "version": 1 }))).await;
    assert_eq!(moved["done_at"], Value::Null);
    let due: Timestamp = moved["due_at"].as_str().unwrap().parse().unwrap();
    assert!(due > Timestamp::now());
    assert!(
        due < Timestamp::now()
            .checked_add(SignedDuration::from_hours(25))
            .unwrap()
    );
    let local = due.to_zoned(jiff::tz::db().get("Europe/Paris").unwrap());
    assert_eq!((local.hour(), local.minute()), (9, 0));

    let (status, _) = s.send("POST", &path, Some(json!({ "version": 1 }))).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn snooze() {
    let s = server().await;
    let r = s
        .create(json!({ "title": "a", "due_at": "2026-11-13T09:00:00Z" }))
        .await;
    let path = format!("/v1/reminders/{}/snooze", r["id"].as_str().unwrap());
    let until = in_minutes(30);
    let (status, r) = s.send("POST", &path, Some(json!({ "until": until }))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(r["snoozed_until"], until);
    for body in [
        json!({}),
        json!({ "minutes": 0 }),
        json!({ "minutes": 10, "until": in_minutes(30) }),
        json!({ "until": "2099-01-01T00:00:00Z" }),
    ] {
        let (status, _) = s.send("POST", &path, Some(body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
    let done = path.replace("snooze", "done");
    s.send("POST", &done, None).await;
    let (status, _) = s.send("POST", &path, Some(json!({ "minutes": 5 }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn delete_leaves_a_tombstone() {
    let s = server().await;
    let r = s
        .create(json!({ "title": "secret", "notes": "n", "due_at": "2026-11-13T09:00:00Z" }))
        .await;
    let path = format!("/v1/reminders/{}", r["id"].as_str().unwrap());
    let (status, _) = s.send("DELETE", &format!("{path}?version=7"), None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = s.send("DELETE", &path, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = s.send("DELETE", &path, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, gone) = s.send("GET", &path, None).await;
    assert_eq!(gone["deleted"], true);
    assert_eq!(gone["title"], "");
    assert_eq!(gone["notes"], Value::Null);
    assert_eq!(gone["version"], 2);
    let (status, _) = s
        .send("PATCH", &path, Some(json!({ "version": 2, "title": "x" })))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, page) = s.send("GET", "/v1/reminders", None).await;
    assert!(page["reminders"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn sync_with_a_cursor() {
    let s = server().await;
    let a = s
        .create(json!({ "title": "a", "due_at": "2026-11-13T09:00:00Z" }))
        .await;
    s.create(json!({ "title": "b", "due_at": "2026-11-12T09:00:00Z" }))
        .await;
    s.create(json!({ "title": "c", "due_at": "2026-11-11T09:00:00Z" }))
        .await;
    s.send(
        "DELETE",
        &format!("/v1/reminders/{}", a["id"].as_str().unwrap()),
        None,
    )
    .await;

    let (_, page) = s.send("GET", "/v1/reminders?since=0&limit=2", None).await;
    let titles: Vec<_> = page["reminders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].clone())
        .collect();
    assert_eq!(
        titles,
        ["b", "c"],
        "changes in order, a moved to its deletion"
    );
    assert_eq!(page["has_more"], true);
    let cursor = page["cursor"].as_i64().unwrap();

    let (_, page) = s
        .send("GET", &format!("/v1/reminders?since={cursor}"), None)
        .await;
    assert_eq!(page["has_more"], false);
    assert_eq!(page["reminders"].as_array().unwrap().len(), 1);
    assert_eq!(page["reminders"][0]["deleted"], true);
    let cursor = page["cursor"].as_i64().unwrap();

    let (_, page) = s
        .send("GET", &format!("/v1/reminders?since={cursor}"), None)
        .await;
    assert!(page["reminders"].as_array().unwrap().is_empty());
    assert_eq!(page["cursor"].as_i64().unwrap(), cursor);

    let (_, page) = s.send("GET", "/v1/reminders", None).await;
    assert_eq!(page["cursor"].as_i64().unwrap(), cursor);
}

#[tokio::test]
async fn events_announce_changes_and_due_times() {
    let s = server().await;
    let req = Request::get("/v1/events")
        .header("authorization", format!("Bearer {}", s.token))
        .body(Body::empty())
        .unwrap();
    let res = s.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let mut stream = res.into_body().into_data_stream();

    let due = Timestamp::now()
        .checked_add(SignedDuration::from_secs(2))
        .unwrap()
        .round(jiff::Unit::Second)
        .unwrap();
    s.create(json!({ "title": "due now", "due_at": due.to_string() }))
        .await;
    s.create(json!({ "title": "later", "due_at": in_minutes(60) }))
        .await;

    let mut seen = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while !seen.contains("event: due") {
        let chunk = tokio::time::timeout_at(deadline, stream.next())
            .await
            .expect("a due event in time")
            .unwrap()
            .unwrap();
        seen.push_str(std::str::from_utf8(&chunk).unwrap());
    }
    assert!(seen.contains("event: changed"));
    let due_line = seen
        .split("event: due\n")
        .nth(1)
        .unwrap()
        .lines()
        .next()
        .unwrap();
    assert!(due_line.contains("\"title\":\"due now\""), "{due_line}");
    assert!(Timestamp::now() >= due);
}
