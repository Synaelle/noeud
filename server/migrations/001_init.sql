CREATE TABLE tokens (
  name TEXT PRIMARY KEY,
  hash BLOB NOT NULL UNIQUE,
  created_at INTEGER NOT NULL,
  last_used_at INTEGER
) STRICT;

CREATE TABLE reminders (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  notes TEXT,
  due_at INTEGER NOT NULL,
  time_zone TEXT NOT NULL,
  repeat_every TEXT,
  repeat_interval INTEGER,
  repeat_start INTEGER,
  snoozed_until INTEGER,
  done_at INTEGER,
  deleted INTEGER NOT NULL DEFAULT 0,
  version INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  created_by TEXT NOT NULL,
  change INTEGER NOT NULL UNIQUE
) STRICT;

CREATE INDEX reminders_due ON reminders (coalesce(snoozed_until, due_at))
  WHERE deleted = 0 AND done_at IS NULL;
