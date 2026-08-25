//! Validated safe-journal and archive boundary values.

use std::time::{SystemTime, UNIX_EPOCH};

use fedi_decentralized_service_fleet_manager::{
    MAX_SAFE_EVENT_BATCH_BYTES, SafeEventCursor, SafeEventJournalIncarnation,
};

const MAX_RECORDS_PER_BATCH: usize = 4096;
const MAX_RECORD_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("invalid safe-journal value")]
pub(crate) struct JournalValueError;

/// Collector-generated opaque archive path component.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct JournalStreamId(String);

impl JournalStreamId {
    pub(crate) fn parse(value: String) -> Result<Self, JournalValueError> {
        if value.len() == 32
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        {
            Ok(Self(value))
        } else {
            Err(JournalValueError)
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// UTC reception date encoded as `YYYY-MM-DD`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct ReceptionDay(String);

impl ReceptionDay {
    pub(crate) fn parse(value: String) -> Result<Self, JournalValueError> {
        let bytes = value.as_bytes();
        let shaped = bytes.len() == 10
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes
                .iter()
                .enumerate()
                .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit());
        if !shaped {
            return Err(JournalValueError);
        }
        let year: u16 = value[0..4].parse().map_err(|_| JournalValueError)?;
        let month: u8 = value[5..7].parse().map_err(|_| JournalValueError)?;
        let day: u8 = value[8..10].parse().map_err(|_| JournalValueError)?;
        let leap =
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let maximum = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return Err(JournalValueError),
        };
        if day == 0 || day > maximum {
            return Err(JournalValueError);
        }
        Ok(Self(value))
    }

    /// Convert Unix-epoch seconds to a UTC `YYYY-MM-DD` date.
    ///
    /// This accepts dates in years 0000 through 9999 and fails when checked civil-date
    /// arithmetic or that output range would be exceeded.
    pub(crate) fn from_unix_seconds(seconds: i64) -> Result<Self, JournalValueError> {
        let days = seconds.div_euclid(86_400);
        let z = days.checked_add(719_468).ok_or(JournalValueError)?;
        let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
        let day_of_era = z - era * 146_097;
        let year_of_era =
            (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let mut year = year_of_era + era * 400;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_piece = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * month_piece + 2) / 5 + 1;
        let month = month_piece + if month_piece < 10 { 3 } else { -9 };
        year += i64::from(month <= 2);
        if !(0..=9999).contains(&year) {
            return Err(JournalValueError);
        }
        Ok(Self(format!("{year:04}-{month:02}-{day:02}")))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Exact source JSONL plus source position, constructed only after bounded validation.
pub(crate) struct ValidatedJournalBatch {
    jsonl: Vec<u8>,
    next_cursor: Option<SafeEventCursor>,
    continuity_gap: bool,
}

impl ValidatedJournalBatch {
    pub(crate) fn new(
        expected_incarnation: &SafeEventJournalIncarnation,
        current_cursor: Option<&SafeEventCursor>,
        incarnation: SafeEventJournalIncarnation,
        jsonl: Vec<u8>,
        next_cursor: Option<SafeEventCursor>,
        continuity_gap: bool,
    ) -> Result<Self, JournalValueError> {
        if incarnation != *expected_incarnation
            || jsonl.len() > MAX_SAFE_EVENT_BATCH_BYTES
            || (!jsonl.is_empty() && !jsonl.ends_with(b"\n"))
            || next_cursor
                .as_ref()
                .is_some_and(|cursor| cursor.incarnation != incarnation)
            || next_cursor.as_ref().is_some_and(|cursor| {
                i64::try_from(cursor.segment).is_err() || i64::try_from(cursor.offset).is_err()
            })
            || (!jsonl.is_empty() && next_cursor.is_none())
        {
            return Err(JournalValueError);
        }
        if !continuity_gap {
            match (current_cursor, next_cursor.as_ref(), jsonl.is_empty()) {
                (Some(current), Some(next), false)
                    if (next.segment, next.offset) <= (current.segment, current.offset) =>
                {
                    return Err(JournalValueError);
                }
                (current, next, true) if current != next => return Err(JournalValueError),
                _ => {}
            }
        }
        for (index, record) in jsonl.split_inclusive(|byte| *byte == b'\n').enumerate() {
            if index >= MAX_RECORDS_PER_BATCH || record.len() > MAX_RECORD_BYTES {
                return Err(JournalValueError);
            }
            let value: serde_json::Value = serde_json::from_slice(&record[..record.len() - 1])
                .map_err(|_| JournalValueError)?;
            if value
                .get("fields")
                .and_then(|fields| fields.get("safe_to_share"))
                != Some(&serde_json::Value::Bool(true))
                || value.get("span").is_some()
                || value.get("spans").is_some()
            {
                return Err(JournalValueError);
            }
        }
        Ok(Self {
            jsonl,
            next_cursor,
            continuity_gap,
        })
    }

    pub(crate) fn jsonl(&self) -> &[u8] {
        &self.jsonl
    }
    pub(crate) fn next_cursor(&self) -> Option<&SafeEventCursor> {
        self.next_cursor.as_ref()
    }
    pub(crate) fn continuity_gap(&self) -> bool {
        self.continuity_gap
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.jsonl.is_empty()
    }
}

/// Return current Unix-epoch seconds.
///
/// The result is whole seconds since 1970-01-01T00:00:00Z. This fails if the system
/// clock predates the epoch or its unsigned seconds do not fit in `i64`.
pub(crate) fn unix_seconds() -> Result<i64, JournalValueError> {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| JournalValueError)?
            .as_secs(),
    )
    .map_err(|_| JournalValueError)
}

#[cfg(test)]
#[path = "journal_types_tests.rs"]
mod tests;
