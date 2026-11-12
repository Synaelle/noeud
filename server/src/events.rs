// SPDX-License-Identifier: GPL-3.0-or-later

//! Live events for `GET /v1/events`: every write, and `due` when a
//! reminder's time comes. The scheduler is one task that sleeps until the
//! next due time and wakes early when a write may have moved it. It only
//! announces times that pass while the server runs; clients catch up on the
//! rest with a list.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use jiff::Timestamp;
use serde_json::json;
use tokio::sync::{Notify, broadcast, watch};
use tokio_stream::wrappers::{BroadcastStream, WatchStream};
use tokio_stream::{Stream, StreamExt};

use crate::db::Db;
use crate::reminders::{self, Reminder};

/// Events kept for a slow listener before it misses some.
const BACKLOG: usize = 256;

/// The scheduler checks at least this often, in case the clock jumps.
const LONGEST_SLEEP: Duration = Duration::from_secs(300);

/// Something that happened to a reminder.
#[derive(Debug, Clone)]
pub enum Event {
    /// Created or changed.
    Changed(Reminder),
    /// Deleted.
    Deleted {
        /// The reminder.
        id: String,
        /// Its tombstone's version.
        version: i64,
    },
    /// Its time came.
    Due(Reminder),
}

impl Event {
    fn to_sse(&self) -> SseEvent {
        let (name, data) = match self {
            Self::Changed(r) => ("changed", serde_json::to_string(r)),
            Self::Deleted { id, version } => (
                "deleted",
                Ok(json!({ "id": id, "version": version }).to_string()),
            ),
            Self::Due(r) => ("due", serde_json::to_string(r)),
        };
        SseEvent::default()
            .event(name)
            .data(data.unwrap_or_default())
    }
}

/// Where writes are announced.
#[derive(Clone)]
pub struct Hub {
    events: broadcast::Sender<Event>,
    wake: Arc<Notify>,
}

impl Default for Hub {
    fn default() -> Self {
        Self {
            events: broadcast::channel(BACKLOG).0,
            wake: Arc::new(Notify::new()),
        }
    }
}

impl Hub {
    /// Tells listeners and wakes the scheduler, whose next time may have
    /// moved.
    pub fn send(&self, event: Event) {
        // no listener is fine
        let _ = self.events.send(event);
        self.wake.notify_one();
    }

    /// The event stream for one listener, which ends when `shutdown` turns
    /// true so the server can stop.
    pub fn stream(
        &self,
        shutdown: watch::Receiver<bool>,
    ) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>> + use<>> {
        enum Item {
            Event(Box<Event>),
            Missed,
            Stop,
        }
        let stop = WatchStream::new(shutdown)
            .filter(|stopping| *stopping)
            .map(|_| Item::Stop);
        let stream = BroadcastStream::new(self.events.subscribe())
            .map(|e| e.map_or(Item::Missed, |e| Item::Event(Box::new(e))))
            .merge(stop)
            .take_while(|item| !matches!(item, Item::Stop))
            .filter_map(|item| match item {
                Item::Event(e) => Some(Ok(e.to_sse())),
                Item::Missed | Item::Stop => None,
            });
        Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(30)))
    }

    /// Announces `due` events until `shutdown` turns true.
    pub async fn run_scheduler(self, db: Db, mut shutdown: watch::Receiver<bool>) {
        let mut since = Timestamp::now().as_millisecond();
        loop {
            let now = Timestamp::now();
            let upto = reminders::now();
            let checked = db
                .call(move |tx| {
                    let due = reminders::due_between(tx, since, upto)?;
                    let next = reminders::next_due_after(tx, upto)?;
                    Ok::<_, crate::db::DbError>((due, next))
                })
                .await;
            let wait = match checked {
                Ok((due, next)) => {
                    for r in due {
                        let _ = self.events.send(Event::Due(r));
                    }
                    since = upto.as_second() * 1000 + 999;
                    next.map_or(LONGEST_SLEEP, |at| {
                        let ms = (at * 1000 - now.as_millisecond()).max(0);
                        Duration::from_millis(ms.unsigned_abs()).min(LONGEST_SLEEP)
                    })
                }
                Err(e) => {
                    tracing::error!("due reminders: {e}");
                    Duration::from_secs(5)
                }
            };
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = self.wake.notified() => {}
                _ = shutdown.wait_for(|stopping| *stopping) => return,
            }
        }
    }
}
