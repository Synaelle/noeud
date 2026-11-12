// SPDX-License-Identifier: GPL-3.0-or-later

//! App tokens: `noeud_` and 32 random bytes in base32, shown once when made.
//! Only their SHA-256 is stored, so a copy of the database lets nobody in.
//! Each token has a name, the app it was made for, which is recorded on the
//! reminders that app creates.

use data_encoding::BASE32_NOPAD;
use rusqlite::{OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};

use crate::db::DbError;

const PREFIX: &str = "noeud_";

/// `last_used_at` is written at most this often, so reads stay reads.
const LAST_USED_STEP: i64 = 60;

/// A token as `token list` shows it.
pub struct TokenInfo {
    /// The app it was made for.
    pub name: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds, to the minute.
    pub last_used_at: Option<i64>,
}

/// Whether `name` can name a token: 1 to 64 of a-z, 0-9, dot, dash and
/// underscore.
pub fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Makes a token named `name` and returns it, or `None` when the name is
/// taken.
///
/// # Errors
///
/// The database, or no randomness from the system.
pub fn create(tx: &Transaction<'_>, name: &str, now: i64) -> Result<Option<String>, DbError> {
    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret).map_err(|e| DbError::Task(format!("randomness: {e}")))?;
    let token = format!(
        "{PREFIX}{}",
        BASE32_NOPAD.encode(&secret).to_ascii_lowercase()
    );
    let made = tx.execute(
        "INSERT INTO tokens (name, hash, created_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (name) DO NOTHING",
        params![name, hash(&token), now],
    )?;
    Ok((made == 1).then_some(token))
}

/// Every token, by name.
///
/// # Errors
///
/// The database.
pub fn list(tx: &Transaction<'_>) -> Result<Vec<TokenInfo>, DbError> {
    let mut stmt = tx.prepare("SELECT name, created_at, last_used_at FROM tokens ORDER BY name")?;
    let rows = stmt.query_map([], |r| {
        Ok(TokenInfo {
            name: r.get(0)?,
            created_at: r.get(1)?,
            last_used_at: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Removes the token named `name`. Returns whether there was one.
///
/// # Errors
///
/// The database.
pub fn revoke(tx: &Transaction<'_>, name: &str) -> Result<bool, DbError> {
    Ok(tx.execute("DELETE FROM tokens WHERE name = ?1", [name])? == 1)
}

/// The name of the token `token`, when the server knows it, and notes that
/// it was used.
///
/// # Errors
///
/// The database.
pub fn verify(tx: &Transaction<'_>, token: &str, now: i64) -> Result<Option<String>, DbError> {
    if !token.starts_with(PREFIX) {
        return Ok(None);
    }
    let hash = hash(token);
    let name: Option<String> = tx
        .query_row("SELECT name FROM tokens WHERE hash = ?1", [&hash], |r| {
            r.get(0)
        })
        .optional()?;
    if name.is_some() {
        tx.execute(
            "UPDATE tokens SET last_used_at = ?1
             WHERE hash = ?2 AND (last_used_at IS NULL OR last_used_at <= ?1 - ?3)",
            params![now, hash, LAST_USED_STEP],
        )?;
    }
    Ok(name)
}
