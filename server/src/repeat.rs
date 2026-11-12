// SPDX-License-Identifier: GPL-3.0-or-later

//! Repeating reminders. Occurrence n is the start plus n times the interval,
//! counted on the calendar of the reminder's time zone, always from the
//! start: a monthly reminder on the 31st falls on the last day of shorter
//! months and comes back to the 31st after. A time that falls in a clock
//! change gap moves forward, a repeated hour takes its first pass, as
//! `Temporal` does in the browser, so the extension agrees with the server.

use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use jiff::{Span, Timestamp};
use serde::{Deserialize, Serialize};

/// The unit a reminder repeats by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Every {
    /// Calendar days, not 24 hours.
    Day,
    /// Seven calendar days.
    Week,
    /// Calendar months.
    Month,
    /// Calendar years.
    Year,
}

impl Every {
    /// The name stored in the database and sent in JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }

    /// The reverse of [`Every::as_str`].
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "day" => Self::Day,
            "week" => Self::Week,
            "month" => Self::Month,
            "year" => Self::Year,
            _ => return None,
        })
    }
}

/// Largest interval accepted.
pub const MAX_INTERVAL: i64 = 1000;

/// Occurrence number `n`, or `None` past the calendar's end.
fn occurrence(start: DateTime, tz: &TimeZone, every: Every, step: i64) -> Option<Timestamp> {
    let span = match every {
        Every::Day => Span::new().try_days(step),
        Every::Week => Span::new().try_weeks(step),
        Every::Month => Span::new().try_months(step),
        Every::Year => Span::new().try_years(step),
    }
    .ok()?;
    let civil = start.checked_add(span).ok()?;
    Some(civil.to_zoned(tz.clone()).ok()?.timestamp())
}

/// The first occurrence strictly after `after`, `None` past the year 9999.
pub fn next_after(
    start: Timestamp,
    tz: &TimeZone,
    every: Every,
    interval: i64,
    after: Timestamp,
) -> Option<Timestamp> {
    let begin = start.to_zoned(tz.clone()).datetime();
    // a guess one occurrence short of the answer, so the walk below is short
    // even years after the start
    let guess = match every {
        Every::Day | Every::Week => {
            let unit = if every == Every::Day { 86_400 } else { 604_800 };
            (after.as_second() - start.as_second()) / (unit * interval)
        }
        Every::Month | Every::Year => {
            let end = after.to_zoned(tz.clone()).datetime();
            let months = (i64::from(end.year()) - i64::from(begin.year())) * 12
                + i64::from(end.month())
                - i64::from(begin.month());
            months
                / if every == Every::Month {
                    interval
                } else {
                    interval * 12
                }
        }
    };
    let mut n = (guess - 1).max(0);
    loop {
        let at = occurrence(begin, tz, every, n.checked_mul(interval)?)?;
        if at > after {
            return Some(at);
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Name, start, time zone, unit, interval, after, and the next occurrence. The browser
    /// extension runs the same cases against its own code, so both agree.
    const CASES: &[(&str, &str, &str, Every, i64, &str, &str)] = &[
        (
            "daily, strictly after an occurrence",
            "2026-01-01T09:00:00Z",
            "UTC",
            Every::Day,
            1,
            "2026-01-05T09:00:00Z",
            "2026-01-06T09:00:00Z",
        ),
        (
            "daily, between two occurrences",
            "2026-01-01T09:00:00Z",
            "UTC",
            Every::Day,
            1,
            "2026-01-05T10:00:00Z",
            "2026-01-06T09:00:00Z",
        ),
        (
            "before the start gives the start",
            "2026-01-01T09:00:00Z",
            "UTC",
            Every::Day,
            1,
            "2025-12-01T00:00:00Z",
            "2026-01-01T09:00:00Z",
        ),
        (
            "ten years later",
            "2026-01-01T09:00:00Z",
            "UTC",
            Every::Day,
            1,
            "2036-01-01T09:00:00Z",
            "2036-01-02T09:00:00Z",
        ),
        (
            "every three days in Tokyo",
            "2026-11-01T00:00:00Z",
            "Asia/Tokyo",
            Every::Day,
            3,
            "2026-11-05T00:00:00Z",
            "2026-11-07T00:00:00Z",
        ),
        (
            "daily across the spring change in Paris",
            "2026-03-27T08:00:00Z",
            "Europe/Paris",
            Every::Day,
            1,
            "2026-03-28T08:00:00Z",
            "2026-03-29T07:00:00Z",
        ),
        (
            "daily across the autumn change in Paris",
            "2026-10-23T07:00:00Z",
            "Europe/Paris",
            Every::Day,
            1,
            "2026-10-24T07:00:00Z",
            "2026-10-25T08:00:00Z",
        ),
        (
            "weekly across the spring change in Paris",
            "2026-03-23T08:00:00Z",
            "Europe/Paris",
            Every::Week,
            1,
            "2026-03-23T08:00:00Z",
            "2026-03-30T07:00:00Z",
        ),
        (
            "a time inside the spring gap moves forward",
            "2026-03-07T07:30:00Z",
            "America/New_York",
            Every::Day,
            1,
            "2026-03-07T08:00:00Z",
            "2026-03-08T07:30:00Z",
        ),
        (
            "the day after the spring gap is back to the same time",
            "2026-03-07T07:30:00Z",
            "America/New_York",
            Every::Day,
            1,
            "2026-03-08T07:30:00Z",
            "2026-03-09T06:30:00Z",
        ),
        (
            "a repeated hour in autumn takes the first one",
            "2026-10-31T05:30:00Z",
            "America/New_York",
            Every::Day,
            1,
            "2026-10-31T06:00:00Z",
            "2026-11-01T05:30:00Z",
        ),
        (
            "every two weeks",
            "2026-11-02T18:30:00Z",
            "UTC",
            Every::Week,
            2,
            "2026-11-10T00:00:00Z",
            "2026-11-16T18:30:00Z",
        ),
        (
            "monthly on the 31st, February",
            "2026-01-31T12:00:00Z",
            "UTC",
            Every::Month,
            1,
            "2026-02-01T00:00:00Z",
            "2026-02-28T12:00:00Z",
        ),
        (
            "monthly on the 31st, back to the 31st",
            "2026-01-31T12:00:00Z",
            "UTC",
            Every::Month,
            1,
            "2026-02-28T12:00:00Z",
            "2026-03-31T12:00:00Z",
        ),
        (
            "monthly on the 31st, April",
            "2026-01-31T12:00:00Z",
            "UTC",
            Every::Month,
            1,
            "2026-03-31T12:00:00Z",
            "2026-04-30T12:00:00Z",
        ),
        (
            "every three months",
            "2026-01-15T08:00:00Z",
            "UTC",
            Every::Month,
            3,
            "2026-05-01T00:00:00Z",
            "2026-07-15T08:00:00Z",
        ),
        (
            "yearly on the 29th of February, common year",
            "2024-02-29T10:00:00Z",
            "UTC",
            Every::Year,
            1,
            "2024-03-01T00:00:00Z",
            "2025-02-28T10:00:00Z",
        ),
        (
            "yearly on the 29th of February, leap year",
            "2024-02-29T10:00:00Z",
            "UTC",
            Every::Year,
            1,
            "2027-03-01T00:00:00Z",
            "2028-02-29T10:00:00Z",
        ),
    ];

    #[test]
    fn known_cases() {
        for &(name, start, zone, every, interval, after, next) in CASES {
            let tz = jiff::tz::db().get(zone).expect("zone");
            let start: Timestamp = start.parse().expect("start");
            let after: Timestamp = after.parse().expect("after");
            let next: Timestamp = next.parse().expect("next");
            assert_eq!(
                next_after(start, &tz, every, interval, after),
                Some(next),
                "{name}"
            );
        }
    }

    #[test]
    fn stops_at_the_end_of_the_calendar() {
        let start: Timestamp = "9999-01-01T00:00:00Z".parse().unwrap();
        let after: Timestamp = "9999-06-01T00:00:00Z".parse().unwrap();
        assert_eq!(
            next_after(start, &TimeZone::UTC, Every::Year, 1, after),
            None
        );
    }
}
