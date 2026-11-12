// SPDX-License-Identifier: GPL-3.0-or-later

//! The database: one SQLite file in WAL mode. Migrations are numbered SQL
//! files applied in order and tracked with `PRAGMA user_version`. Calls run
//! on tokio's blocking pool so a slow disk never stalls the async side. One
//! connection behind a mutex is plenty for one owner's reminders, and it
//! makes every call a transaction nobody else interleaves with.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rusqlite::Connection;

/// Applied in order, the position is the schema version.
const MIGRATIONS: &[&str] = &[include_str!("../migrations/001_init.sql")];

/// Shared handle to the database.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

/// A failed database call.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// SQLite reported an error.
    #[error("database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The file was written by a newer server.
    #[error(
        "the database is at schema {found}, this server knows up to {known}: a newer version \
         wrote it"
    )]
    TooNew {
        /// Version in the file.
        found: usize,
        /// Newest version this build has.
        known: usize,
    },
    /// The blocking task panicked or was cancelled.
    #[error("database task failed: {0}")]
    Task(String),
}

impl Db {
    /// Opens (or creates) the database file and brings its schema up to date.
    ///
    /// # Errors
    ///
    /// The file cannot be opened, a migration fails, or a newer server wrote
    /// it.
    pub fn open(file: &Path) -> Result<Self, DbError> {
        let conn = Connection::open(file)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::setup(conn)
    }

    /// A private database in memory, for tests.
    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self::setup(Connection::open_in_memory().expect("open")).expect("migrate")
    }

    fn setup(mut conn: Connection) -> Result<Self, DbError> {
        // the command line may write while the server runs
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let found: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let found = usize::try_from(found).unwrap_or(usize::MAX);
        if found > MIGRATIONS.len() {
            return Err(DbError::TooNew {
                found,
                known: MIGRATIONS.len(),
            });
        }
        for (done, sql) in MIGRATIONS.iter().enumerate().skip(found) {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(
                None,
                "user_version",
                i64::try_from(done + 1).unwrap_or(i64::MAX),
            )?;
            tx.commit()?;
        }
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Runs `f` on the connection inside one transaction, on the blocking
    /// pool. An error rolls the transaction back.
    ///
    /// # Errors
    ///
    /// Whatever `f` returns, or a failed task.
    pub async fn call<T, E, F>(&self, f: F) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<DbError> + Send + 'static,
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, E> + Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut conn = conn.lock().unwrap_or_else(PoisonError::into_inner);
            let tx = conn.transaction().map_err(DbError::from)?;
            let value = f(&tx)?;
            tx.commit().map_err(DbError::from)?;
            Ok(value)
        })
        .await
        .map_err(|e| E::from(DbError::Task(e.to_string())))?
    }
}
