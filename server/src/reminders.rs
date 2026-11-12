// SPDX-License-Identifier: GPL-3.0-or-later

//! Reminders: what clients send, what is stored, and every change to them.
//! Each write bumps the reminder's `version` and gives it the next change
//! number, which is the sync cursor. Deleted reminders stay as tombstones
//! with their text cleared, so other clients learn about the deletion.

use jiff::Timestamp;
use rusqlite::types::ToSql;
use rusqlite::{OptionalExtension, Row, Transaction, params};
use serde::{Deserialize, Deserializer, Serialize};

use crate::db::DbError;
use crate::error::Error;
use crate::repeat::{self, Every};

const MAX_TITLE: usize = 500;
const MAX_NOTES: usize = 10_000;
const MAX_SNOOZE_MINUTES: i64 = 525_600;
const DEFAULT_LIMIT: u32 = 500;
const MAX_LIMIT: u32 = 1000;

/// A reminder as stored and sent.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reminder {
    /// A UUID, lowercase with hyphens.
    pub id: String,
    /// What to remember, empty once deleted.
    pub title: String,
    /// Optional longer text.
    pub notes: Option<String>,
    /// When it is due, unless snoozed.
    pub due_at: Timestamp,
    /// IANA name the repeats are counted in.
    pub time_zone: String,
    /// How it repeats, if it does.
    pub repeat: Option<Repeat>,
    /// When it is due instead of `due_at`.
    pub snoozed_until: Option<Timestamp>,
    /// When it was marked done.
    pub done_at: Option<Timestamp>,
    /// A tombstone.
    pub deleted: bool,
    /// Goes up by one on every change.
    pub version: i64,
    /// When it was made.
    pub created_at: Timestamp,
    /// When it last changed.
    pub updated_at: Timestamp,
    /// The token that made it.
    pub created_by: String,
    /// The change number of its last write.
    #[serde(skip)]
    pub change: i64,
}

/// How a reminder repeats.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Repeat {
    /// The unit.
    pub every: Every,
    /// How many units between two occurrences.
    pub interval: i64,
    /// The first occurrence, which every other one counts from.
    pub start: Timestamp,
}

/// `repeat` as clients send it.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepeatInput {
    every: Every,
    #[serde(default = "one")]
    interval: i64,
    start: Option<Timestamp>,
}

fn one() -> i64 {
    1
}

/// The body of a create.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewReminder {
    id: Option<String>,
    title: String,
    notes: Option<String>,
    due_at: Timestamp,
    time_zone: Option<String>,
    repeat: Option<RepeatInput>,
}

/// The body of a change: a field left out stays, a field sent as `null` is
/// cleared. The outer option is whether it was sent.
#[allow(clippy::option_option)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Changes {
    version: i64,
    #[serde(default, deserialize_with = "present")]
    title: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    notes: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    due_at: Option<Option<Timestamp>>,
    #[serde(default, deserialize_with = "present")]
    time_zone: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    repeat: Option<Option<RepeatInput>>,
    #[serde(default, deserialize_with = "present")]
    snoozed_until: Option<Option<Timestamp>>,
    #[serde(default, deserialize_with = "present")]
    done_at: Option<Option<Timestamp>>,
}

/// Tells a field sent as `null` from a field left out.
#[allow(clippy::option_option)]
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// The body of a done, optional.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Done {
    version: Option<i64>,
}

/// The body of a snooze.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snooze {
    until: Option<Timestamp>,
    minutes: Option<i64>,
    version: Option<i64>,
}

/// The query of a list.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    done: Option<bool>,
    due_before: Option<Timestamp>,
    since: Option<i64>,
    limit: Option<u32>,
}

/// A page of reminders.
#[derive(Debug, Serialize)]
pub struct Page {
    /// The reminders.
    pub reminders: Vec<Reminder>,
    /// Where the next sync starts.
    pub cursor: i64,
    /// Whether to ask again from `cursor`.
    pub has_more: bool,
}

/// The current time, to the second like everything stored.
pub fn now() -> Timestamp {
    Timestamp::from_second(Timestamp::now().as_second()).expect("the clock is in range")
}

fn to_second(t: Timestamp) -> Timestamp {
    Timestamp::from_second(t.as_second()).expect("in range")
}

fn title(text: &str) -> Result<String, Error> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > MAX_TITLE {
        return Err(Error::invalid(format!(
            "title must be 1 to {MAX_TITLE} characters"
        )));
    }
    Ok(text.to_owned())
}

fn notes(text: Option<String>) -> Result<Option<String>, Error> {
    match text {
        Some(t) if t.chars().count() > MAX_NOTES => Err(Error::invalid(format!(
            "notes must be at most {MAX_NOTES} characters"
        ))),
        Some(t) if t.trim().is_empty() => Ok(None),
        other => Ok(other),
    }
}

fn time_zone(name: &str) -> Result<String, Error> {
    jiff::tz::db()
        .get(name)
        .map(|_| name.to_owned())
        .map_err(|_| Error::invalid(format!("unknown time zone {name}")))
}

fn repeat(input: RepeatInput, due_at: Timestamp) -> Result<Repeat, Error> {
    if !(1..=repeat::MAX_INTERVAL).contains(&input.interval) {
        return Err(Error::invalid(format!(
            "repeat.interval must be 1 to {}",
            repeat::MAX_INTERVAL
        )));
    }
    Ok(Repeat {
        every: input.every,
        interval: input.interval,
        start: to_second(input.start.unwrap_or(due_at)),
    })
}

fn required<T>(field: &str, value: Option<T>) -> Result<T, Error> {
    value.ok_or_else(|| Error::invalid(format!("{field} cannot be null")))
}

fn check_version(current: &Reminder, version: Option<i64>) -> Result<(), Error> {
    match version {
        Some(v) if v != current.version => Err(Error::conflict(current.clone())),
        _ => Ok(()),
    }
}

/// The reminder `id`, or not found. Deleted ones are found too.
fn existing(tx: &Transaction<'_>, id: &str) -> Result<Reminder, Error> {
    get(tx, id)?.ok_or_else(|| Error::not_found("no such reminder"))
}

/// The reminder `id` when it can still change.
fn live(tx: &Transaction<'_>, id: &str) -> Result<Reminder, Error> {
    let reminder = existing(tx, id)?;
    if reminder.deleted {
        return Err(Error::not_found("the reminder was deleted"));
    }
    Ok(reminder)
}

/// A UUID in its usual form, or why not.
pub fn normal_id(id: &str) -> Result<String, Error> {
    uuid::Uuid::parse_str(id)
        .map(|u| u.hyphenated().to_string())
        .map_err(|_| Error::invalid("id must be a UUID"))
}

/// Creates a reminder. Returns it and whether it is new: an id that exists
/// returns the stored reminder unchanged.
///
/// # Errors
///
/// Invalid fields, the database.
pub fn create(
    tx: &Transaction<'_>,
    input: NewReminder,
    by: &str,
) -> Result<(Reminder, bool), Error> {
    let id = match input.id {
        Some(id) => normal_id(&id)?,
        None => uuid::Uuid::now_v7().hyphenated().to_string(),
    };
    if let Some(stored) = get(tx, &id)? {
        return Ok((stored, false));
    }
    let now = now();
    let due_at = to_second(input.due_at);
    let reminder = Reminder {
        id,
        title: title(&input.title)?,
        notes: notes(input.notes)?,
        due_at,
        time_zone: time_zone(input.time_zone.as_deref().unwrap_or("UTC"))?,
        repeat: input.repeat.map(|r| repeat(r, due_at)).transpose()?,
        snoozed_until: None,
        done_at: None,
        deleted: false,
        version: 1,
        created_at: now,
        updated_at: now,
        created_by: by.to_owned(),
        change: 0,
    };
    Ok((write(tx, reminder)?, true))
}

/// Applies `changes` to the reminder `id`.
///
/// # Errors
///
/// Not found, deleted, a stale version, invalid fields, the database.
pub fn change(tx: &Transaction<'_>, id: &str, changes: Changes) -> Result<Reminder, Error> {
    let mut r = live(tx, id)?;
    check_version(&r, Some(changes.version))?;
    if let Some(t) = changes.title {
        r.title = title(&required("title", t)?)?;
    }
    if let Some(n) = changes.notes {
        r.notes = notes(n)?;
    }
    if let Some(z) = changes.time_zone {
        r.time_zone = time_zone(&required("time_zone", z)?)?;
    }
    if let Some(due) = changes.due_at {
        let due = to_second(required("due_at", due)?);
        if due != r.due_at {
            r.due_at = due;
            r.snoozed_until = None;
            if let Some(rep) = &mut r.repeat {
                rep.start = due;
            }
        }
    }
    if let Some(rep) = changes.repeat {
        r.repeat = rep.map(|rep| repeat(rep, r.due_at)).transpose()?;
    }
    if let Some(s) = changes.snoozed_until {
        r.snoozed_until = s.map(to_second);
    }
    if let Some(d) = changes.done_at {
        r.done_at = d.map(to_second);
    }
    Ok(write(tx, r)?)
}

/// Marks a reminder done, or moves a repeating one to its next occurrence.
///
/// # Errors
///
/// Not found, deleted, a stale version, no occurrence left, the database.
pub fn done(tx: &Transaction<'_>, id: &str, body: Done) -> Result<Reminder, Error> {
    let mut r = live(tx, id)?;
    check_version(&r, body.version)?;
    let now = now();
    match &r.repeat {
        Some(rep) => {
            let tz = jiff::tz::db()
                .get(&r.time_zone)
                .map_err(|e| Error::internal(&format!("stored time zone: {e}")))?;
            let after = now.max(r.due_at);
            r.due_at = repeat::next_after(rep.start, &tz, rep.every, rep.interval, after)
                .ok_or_else(|| Error::invalid("the reminder has no occurrence left"))?;
        }
        None => r.done_at = Some(now),
    }
    r.snoozed_until = None;
    Ok(write(tx, r)?)
}

/// Snoozes a reminder.
///
/// # Errors
///
/// Not found, deleted, done, a stale version, a bad time, the database.
pub fn snooze(tx: &Transaction<'_>, id: &str, body: Snooze) -> Result<Reminder, Error> {
    let mut r = live(tx, id)?;
    check_version(&r, body.version)?;
    if r.done_at.is_some() {
        return Err(Error::invalid("the reminder is done"));
    }
    let now = now();
    let latest = now
        .checked_add(jiff::SignedDuration::from_mins(MAX_SNOOZE_MINUTES))
        .expect("in range");
    let until = match (body.until, body.minutes) {
        (Some(until), None) => to_second(until),
        (None, Some(m)) if (1..=MAX_SNOOZE_MINUTES).contains(&m) => now
            .checked_add(jiff::SignedDuration::from_mins(m))
            .expect("in range"),
        (None, Some(_)) => {
            return Err(Error::invalid(format!(
                "minutes must be 1 to {MAX_SNOOZE_MINUTES}"
            )));
        }
        _ => return Err(Error::invalid("send either until or minutes")),
    };
    if until > latest {
        return Err(Error::invalid("a snooze lasts a year at most"));
    }
    r.snoozed_until = Some(until);
    Ok(write(tx, r)?)
}

/// Deletes a reminder. Returns the tombstone, or `None` when it was already
/// deleted.
///
/// # Errors
///
/// Not found, a stale version, the database.
pub fn delete(
    tx: &Transaction<'_>,
    id: &str,
    version: Option<i64>,
) -> Result<Option<Reminder>, Error> {
    let mut r = existing(tx, id)?;
    if r.deleted {
        return Ok(None);
    }
    check_version(&r, version)?;
    r.deleted = true;
    r.title = String::new();
    r.notes = None;
    Ok(Some(write(tx, r)?))
}

const COLUMNS: &str = "id, title, notes, due_at, time_zone, repeat_every, repeat_interval, \
    repeat_start, snoozed_until, done_at, deleted, version, created_at, updated_at, created_by, \
    change";

fn time(second: i64) -> rusqlite::Result<Timestamp> {
    Timestamp::from_second(second).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Integer, Box::new(e))
    })
}

fn read(row: &Row<'_>) -> rusqlite::Result<Reminder> {
    let every: Option<String> = row.get(5)?;
    let repeat = match every {
        Some(every) => Some(Repeat {
            every: Every::parse(&every).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    format!("repeat unit {every}").into(),
                )
            })?,
            interval: row.get(6)?,
            start: time(row.get(7)?)?,
        }),
        None => None,
    };
    Ok(Reminder {
        id: row.get(0)?,
        title: row.get(1)?,
        notes: row.get(2)?,
        due_at: time(row.get(3)?)?,
        time_zone: row.get(4)?,
        repeat,
        snoozed_until: row.get::<_, Option<i64>>(8)?.map(time).transpose()?,
        done_at: row.get::<_, Option<i64>>(9)?.map(time).transpose()?,
        deleted: row.get(10)?,
        version: row.get(11)?,
        created_at: time(row.get(12)?)?,
        updated_at: time(row.get(13)?)?,
        created_by: row.get(14)?,
        change: row.get(15)?,
    })
}

/// The reminder `id`, deleted ones included.
///
/// # Errors
///
/// The database.
pub fn get(tx: &Transaction<'_>, id: &str) -> Result<Option<Reminder>, DbError> {
    Ok(tx
        .query_row(
            &format!("SELECT {COLUMNS} FROM reminders WHERE id = ?1"),
            [id],
            read,
        )
        .optional()?)
}

fn cursor(tx: &Transaction<'_>) -> Result<i64, DbError> {
    Ok(
        tx.query_row("SELECT coalesce(max(change), 0) FROM reminders", [], |r| {
            r.get(0)
        })?,
    )
}

/// Stores `r` as its next version: a new one is inserted at version 1, a
/// known one gets its version bumped.
fn write(tx: &Transaction<'_>, mut r: Reminder) -> Result<Reminder, DbError> {
    let is_new = r.change == 0;
    r.change = cursor(tx)? + 1;
    if is_new {
        r.version = 1;
    } else {
        r.version += 1;
        r.updated_at = now();
    }
    let repeat = r.repeat.as_ref();
    tx.execute(
        &format!(
            "INSERT OR REPLACE INTO reminders ({COLUMNS})
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"
        ),
        params![
            r.id,
            r.title,
            r.notes,
            r.due_at.as_second(),
            r.time_zone,
            repeat.map(|x| x.every.as_str()),
            repeat.map(|x| x.interval),
            repeat.map(|x| x.start.as_second()),
            r.snoozed_until.map(Timestamp::as_second),
            r.done_at.map(Timestamp::as_second),
            r.deleted,
            r.version,
            r.created_at.as_second(),
            r.updated_at.as_second(),
            r.created_by,
            r.change,
        ],
    )?;
    Ok(r)
}

/// A page of reminders for `filter`.
///
/// # Errors
///
/// A limit out of range, the database.
pub fn list(tx: &Transaction<'_>, filter: &Filter) -> Result<Page, Error> {
    let limit = filter.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(Error::invalid(format!("limit must be 1 to {MAX_LIMIT}")));
    }
    let mut sql = format!("SELECT {COLUMNS} FROM reminders WHERE ");
    let mut args: Vec<Box<dyn ToSql>> = Vec::new();
    match filter.since {
        Some(since) => {
            sql.push_str("change > ?");
            args.push(Box::new(since));
        }
        None => sql.push_str("deleted = 0"),
    }
    match filter.done {
        Some(true) => sql.push_str(" AND done_at IS NOT NULL"),
        Some(false) => sql.push_str(" AND done_at IS NULL"),
        None => {}
    }
    if let Some(before) = filter.due_before {
        sql.push_str(" AND coalesce(snoozed_until, due_at) <= ?");
        args.push(Box::new(before.as_second()));
    }
    sql.push_str(if filter.since.is_some() {
        " ORDER BY change"
    } else {
        " ORDER BY coalesce(snoozed_until, due_at), id"
    });
    sql.push_str(" LIMIT ?");
    args.push(Box::new(i64::from(limit) + 1));

    let mut stmt = tx.prepare(&sql).map_err(DbError::from)?;
    let mut reminders = stmt
        .query_map(rusqlite::params_from_iter(args.iter()), read)
        .map_err(DbError::from)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(DbError::from)?;
    let has_more = reminders.len() > limit as usize;
    reminders.truncate(limit as usize);
    let cursor = match (filter.since, has_more, reminders.last()) {
        (Some(_), true, Some(last)) => last.change,
        _ => cursor(tx)?,
    };
    Ok(Page {
        reminders,
        cursor,
        has_more,
    })
}

/// Reminders not done or deleted that are due after `after_ms` (exclusive, in
/// milliseconds) and at or before `upto`, soonest first.
///
/// # Errors
///
/// The database.
pub fn due_between(
    tx: &Transaction<'_>,
    after_ms: i64,
    upto: Timestamp,
) -> Result<Vec<Reminder>, DbError> {
    let mut stmt = tx.prepare(&format!(
        "SELECT {COLUMNS} FROM reminders
         WHERE deleted = 0 AND done_at IS NULL
           AND coalesce(snoozed_until, due_at) > ?1 AND coalesce(snoozed_until, due_at) <= ?2
         ORDER BY coalesce(snoozed_until, due_at)"
    ))?;
    let rows = stmt.query_map(params![after_ms.div_euclid(1000), upto.as_second()], read)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The soonest time after `after` at which a reminder is due.
///
/// # Errors
///
/// The database.
pub fn next_due_after(tx: &Transaction<'_>, after: Timestamp) -> Result<Option<i64>, DbError> {
    Ok(tx.query_row(
        "SELECT min(coalesce(snoozed_until, due_at)) FROM reminders
         WHERE deleted = 0 AND done_at IS NULL AND coalesce(snoozed_until, due_at) > ?1",
        [after.as_second()],
        |r| r.get(0),
    )?)
}
