//! The time of day as PixelIt needs it: seconds since 1970 for records, and
//! the local date and time for the names of files.

use std::time::{SystemTime, UNIX_EPOCH};

use time::{OffsetDateTime, UtcOffset};

/// Seconds since the first of January 1970, UTC.
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

/// A moment in the time zone of this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalTime {
    /// Seconds since the first of January 1970, UTC.
    pub timestamp: i64,
    /// `2026-01-31`
    pub date: String,
    /// `23-59-58`; a file name cannot hold a colon.
    pub time: String,
}

impl LocalTime {
    pub fn now() -> Self {
        let utc = OffsetDateTime::now_utc();
        // A computer that cannot tell its time zone gets the time in UTC.
        let offset = UtcOffset::local_offset_at(utc).unwrap_or(UtcOffset::UTC);
        Self::of(utc.to_offset(offset))
    }

    fn of(moment: OffsetDateTime) -> Self {
        Self {
            timestamp: moment.unix_timestamp(),
            date: format!(
                "{:04}-{:02}-{:02}",
                moment.year(),
                u8::from(moment.month()),
                moment.day()
            ),
            time: format!(
                "{:02}-{:02}-{:02}",
                moment.hour(),
                moment.minute(),
                moment.second()
            ),
        }
    }

    /// `2026-01-31 23-59-58`, for the names of captured and pasted pictures.
    pub fn stamp(&self) -> String {
        format!("{} {}", self.date, self.time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moment_is_written_for_file_names() {
        let moment = OffsetDateTime::from_unix_timestamp(1_769_903_998)
            .expect("a valid timestamp")
            .to_offset(UtcOffset::from_hms(3, 0, 0).expect("a valid offset"));
        let local = LocalTime::of(moment);
        assert_eq!(local.timestamp, 1_769_903_998);
        assert_eq!(local.date, "2026-02-01");
        assert_eq!(local.time, "02-59-58");
        assert_eq!(local.stamp(), "2026-02-01 02-59-58");
    }

    #[test]
    fn now_is_after_the_first_release() {
        assert!(unix_now() > 1_760_000_000);
        assert!((LocalTime::now().timestamp - unix_now()).abs() <= 2);
    }
}
