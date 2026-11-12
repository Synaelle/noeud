// SPDX-License-Identifier: GPL-3.0-or-later

//! Noeud's server: keeps the reminders and serves them to apps that hold a
//! token. The same binary makes and revokes the tokens.

mod auth;
mod cli;
mod db;
mod error;
mod events;
mod reminders;
mod repeat;
mod routes;
#[cfg(test)]
mod tests;
mod tokens;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use axum::http::HeaderValue;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

use crate::db::Db;
use crate::events::Hub;
use crate::routes::AppState;

const USAGE: &str = "usage: noeud-server [--db <file>] [--listen <address>] <command>

  serve               serves the API
  token create <name> makes a token for an app and prints it, once
  token list          lists the tokens and when they were last used
  token revoke <name> removes a token, its app is refused from then on
  healthcheck         exits 0 when the running server answers, for Docker
  version             prints the version

  --db <file>         the database, NOEUD_DB, noeud.db by default
  --listen <address>  where to serve, NOEUD_LISTEN, 127.0.0.1:8470 by default

NOEUD_CORS_ORIGINS lets web pages on those origins use the API, separated by
commas. Browser extensions do not need it. NOEUD_LOG sets the log level.

The token commands work while the server runs, with docker exec too.";

/// Where to find things, from the command line or the environment.
pub struct Settings {
    /// The database file.
    pub db: PathBuf,
    /// The address to serve on.
    pub listen: SocketAddr,
}

fn parse(args: &mut Vec<String>) -> Result<Settings, String> {
    let mut db = std::env::var_os("NOEUD_DB").map_or_else(|| "noeud.db".into(), PathBuf::from);
    let mut listen = std::env::var("NOEUD_LISTEN").unwrap_or_else(|_| "127.0.0.1:8470".into());
    let mut rest = Vec::new();
    let mut words = args.drain(..);
    while let Some(word) = words.next() {
        match word.as_str() {
            "--db" => db = words.next().ok_or("--db needs a file")?.into(),
            "--listen" => listen = words.next().ok_or("--listen needs an address")?,
            _ => rest.push(word),
        }
    }
    drop(words);
    *args = rest;
    let listen = listen
        .parse()
        .map_err(|_| format!("{listen} is not an address like 127.0.0.1:8470"))?;
    Ok(Settings { db, listen })
}

fn cors_origins() -> Result<Vec<HeaderValue>, String> {
    let Ok(list) = std::env::var("NOEUD_CORS_ORIGINS") else {
        return Ok(Vec::new());
    };
    list.split(',')
        .map(str::trim)
        .filter(|o| !o.is_empty())
        .map(|o| HeaderValue::from_str(o).map_err(|_| format!("bad origin {o}")))
        .collect()
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let result = parse(&mut args).and_then(|settings| {
        let words: Vec<&str> = args.iter().map(String::as_str).collect();
        match words.as_slice() {
            ["serve"] => serve(&settings),
            ["token", rest @ ..] => cli::token(&settings, rest),
            ["healthcheck"] => cli::healthcheck(settings.listen),
            ["version"] => {
                println!("noeud-server {}", env!("CARGO_PKG_VERSION"));
                Ok(())
            }
            _ => Err(format!("{USAGE}\n")),
        }
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{}", message.trim_end());
            ExitCode::FAILURE
        }
    }
}

fn serve(settings: &Settings) -> Result<(), String> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("NOEUD_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let origins = cors_origins()?;
    let db = Db::open(&settings.db).map_err(|e| e.to_string())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(settings.listen)
            .await
            .map_err(|e| format!("cannot listen on {}: {e}", settings.listen))?;
        let (stop, shutdown) = watch::channel(false);
        let hub = Hub::default();
        let scheduler = tokio::spawn(hub.clone().run_scheduler(db.clone(), shutdown.clone()));
        let app = routes::app(AppState { db, hub, shutdown }, &origins);
        tracing::info!(
            "noeud-server {} serving on {}, database {}",
            env!("CARGO_PKG_VERSION"),
            settings.listen,
            settings.db.display()
        );
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                stop_signal().await;
                tracing::info!("stopping");
                let _ = stop.send(true);
            })
            .await
            .map_err(|e| e.to_string())?;
        let _ = scheduler.await;
        Ok(())
    })
}

async fn stop_signal() {
    let interrupt = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal handler");
        tokio::select! {
            _ = interrupt => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = interrupt.await;
}
