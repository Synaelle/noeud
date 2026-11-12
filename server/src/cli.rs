// SPDX-License-Identifier: GPL-3.0-or-later

//! Commands for whoever runs the server, on its machine or with
//! `docker exec`. They open the database themselves; SQLite lets them write
//! while the server runs.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::time::Duration;

use jiff::Timestamp;

use crate::Settings;
use crate::db::Db;
use crate::tokens;

fn when(second: i64) -> String {
    Timestamp::from_second(second).map_or_else(|_| second.to_string(), |t| t.to_string())
}

/// `token create|list|revoke`.
pub fn token(settings: &Settings, words: &[&str]) -> Result<(), String> {
    let db = Db::open(&settings.db).map_err(|e| e.to_string())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|e| e.to_string())?;
    let now = crate::reminders::now().as_second();
    runtime.block_on(async {
        match words {
            ["create", name] => {
                if !tokens::valid_name(name) {
                    return Err(
                        "a token name is 1 to 64 of a-z, 0-9, dot, dash and underscore".into(),
                    );
                }
                let owned = (*name).to_owned();
                let made = db
                    .call(move |tx| tokens::create(tx, &owned, now))
                    .await
                    .map_err(|e| e.to_string())?;
                let token = made.ok_or(format!("a token named {name} exists already"))?;
                println!("{token}");
                eprintln!("This is the only time the token is shown. Give it to the app now.");
                Ok(())
            }
            ["list"] => {
                let list = db.call(tokens::list).await.map_err(|e| e.to_string())?;
                if list.is_empty() {
                    eprintln!("No tokens yet, make one with: noeud-server token create <name>");
                }
                for t in list {
                    let used = t.last_used_at.map_or_else(|| "never".into(), when);
                    println!(
                        "{}\tcreated {}\tlast used {used}",
                        t.name,
                        when(t.created_at)
                    );
                }
                Ok(())
            }
            ["revoke", name] => {
                let owned = (*name).to_owned();
                let gone = db
                    .call(move |tx| tokens::revoke(tx, &owned))
                    .await
                    .map_err(|e| e.to_string())?;
                if gone {
                    println!("revoked {name}");
                    Ok(())
                } else {
                    Err(format!("no token named {name}"))
                }
            }
            _ => Err("usage: noeud-server token create <name> | list | revoke <name>".into()),
        }
    })
}

/// Asks the running server's health endpoint, for container health checks
/// where there is no curl.
pub fn healthcheck(listen: SocketAddr) -> Result<(), String> {
    let mut addr = listen;
    if addr.ip().is_unspecified() {
        addr.set_ip(match addr.ip() {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
        });
    }
    let timeout = Duration::from_secs(5);
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    stream
        .write_all(b"GET /v1/health HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .map_err(|e| e.to_string())?;
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).map_err(|e| e.to_string())?;
    if answer.starts_with(b"HTTP/1.0 200") || answer.starts_with(b"HTTP/1.1 200") {
        Ok(())
    } else {
        Err(format!("the server at {addr} did not answer ok"))
    }
}
