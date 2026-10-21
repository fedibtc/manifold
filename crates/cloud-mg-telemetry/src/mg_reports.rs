//! Each MG's newest `seat capacity` and `seat readiness check completed`
//! events, validated while a journal batch is validated and served on the
//! private listener. The MG owns both event shapes
//! ([`SeatCapacityEvent`], [`SeatReadinessEvent`]); this module only bounds
//! them.

use std::time::{Duration, UNIX_EPOCH};

use fedi_decentralized_domain::SETUP_PAYMENT_MAX_SEATS_PER_MG;
use fedi_decentralized_service_manifold_guardian::{
    SEAT_CAPACITY_EVENT, SEAT_READINESS_EVENT, SeatCapacityEvent, SeatReadinessEvent,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

const YEAR_10000_MILLIS: u64 = 253_402_300_800_000;
const MAX_VERSION_BYTES: usize = 64;
const MAX_CODE_BYTES: usize = 32;

/// Which event a stored report came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MgReportKind {
    SeatCapacity,
    SeatReadiness,
}

impl MgReportKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SeatCapacity => "seat_capacity",
            Self::SeatReadiness => "seat_readiness",
        }
    }
}

/// A validated report kept per MG. Its serde form is both the stored row and
/// the export entry, and it decodes through the MG's event type, so the event
/// field names also name the stored row and export fields. It is a persisted
/// format: decoding a stored row validates it again.
pub(crate) trait MgReport: Serialize + DeserializeOwned {
    const KIND: MgReportKind;
    /// The journal message whose fields carry this report.
    const MESSAGE: &'static str;
}

/// An MG's seat limits and free seats, as its `seat capacity` event states
/// them, admitted only when the counts agree with each other.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "SeatCapacityEvent")]
pub(crate) struct SeatCapacity {
    pub(crate) available_seats: u32,
    pub(crate) accepting_seats: bool,
    pub(crate) max_seats: Option<u32>,
    pub(crate) memory_max_seats: Option<u32>,
    pub(crate) policy_max_seats: Option<u32>,
    pub(crate) mg_version: Option<MgVersion>,
}

impl MgReport for SeatCapacity {
    const KIND: MgReportKind = MgReportKind::SeatCapacity;
    const MESSAGE: &'static str = SEAT_CAPACITY_EVENT;
}

impl TryFrom<SeatCapacityEvent> for SeatCapacity {
    type Error = &'static str;

    /// Free seats must not exceed any stated limit, and neither free seats nor
    /// the smallest limit may exceed the most seats a setup-payment policy
    /// allows one MG.
    fn try_from(event: SeatCapacityEvent) -> Result<Self, Self::Error> {
        let SeatCapacityEvent {
            available_seats,
            accepting_seats,
            max_seats,
            memory_max_seats,
            policy_max_seats,
            mg_version,
        } = event;
        let smallest_limit = [max_seats, memory_max_seats, policy_max_seats]
            .into_iter()
            .flatten()
            .min();
        if smallest_limit.is_some_and(|limit| available_seats > limit) {
            return Err("more free seats than a stated limit");
        }
        if smallest_limit.unwrap_or(available_seats) > SETUP_PAYMENT_MAX_SEATS_PER_MG {
            return Err("seat count above the setup-payment maximum");
        }
        Ok(Self {
            available_seats,
            accepting_seats,
            max_seats,
            memory_max_seats,
            policy_max_seats,
            mg_version: mg_version.map(MgVersion::try_from).transpose()?,
        })
    }
}

/// An MG's readiness verdict and uptime scores, as its `seat readiness check
/// completed` event states them.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(try_from = "SeatReadinessEvent")]
pub(crate) struct SeatReadiness {
    pub(crate) ready: bool,
    pub(crate) relay: ReadinessCode,
    pub(crate) discovery: ReadinessCode,
    pub(crate) bitcoin: ReadinessCode,
    pub(crate) uptime: Option<ReadinessCode>,
    pub(crate) uptime_1d_pct: Option<Percent>,
    pub(crate) uptime_7d_pct: Option<Percent>,
    pub(crate) uptime_30d_pct: Option<Percent>,
    pub(crate) min_uptime_1d_pct: Option<Percent>,
    pub(crate) min_uptime_7d_pct: Option<Percent>,
    pub(crate) min_uptime_30d_pct: Option<Percent>,
}

impl MgReport for SeatReadiness {
    const KIND: MgReportKind = MgReportKind::SeatReadiness;
    const MESSAGE: &'static str = SEAT_READINESS_EVENT;
}

impl TryFrom<SeatReadinessEvent> for SeatReadiness {
    type Error = &'static str;

    fn try_from(event: SeatReadinessEvent) -> Result<Self, Self::Error> {
        let SeatReadinessEvent {
            ready,
            relay,
            discovery,
            bitcoin,
            uptime,
            uptime_1d_pct,
            uptime_7d_pct,
            uptime_30d_pct,
            min_uptime_1d_pct,
            min_uptime_7d_pct,
            min_uptime_30d_pct,
        } = event;
        let percent = |value: Option<f64>| value.map(Percent::try_from).transpose();
        Ok(Self {
            ready,
            relay: relay.try_into()?,
            discovery: discovery.try_into()?,
            bitcoin: bitcoin.try_into()?,
            uptime: uptime.map(ReadinessCode::try_from).transpose()?,
            uptime_1d_pct: percent(uptime_1d_pct)?,
            uptime_7d_pct: percent(uptime_7d_pct)?,
            uptime_30d_pct: percent(uptime_30d_pct)?,
            min_uptime_1d_pct: percent(min_uptime_1d_pct)?,
            min_uptime_7d_pct: percent(min_uptime_7d_pct)?,
            min_uptime_30d_pct: percent(min_uptime_30d_pct)?,
        })
    }
}

/// A release version: at most 64 ASCII letters, digits, `.`, `+` and `-`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "String")]
pub(crate) struct MgVersion(String);

impl TryFrom<String> for MgVersion {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let valid = !value.is_empty()
            && value.len() <= MAX_VERSION_BYTES
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-'));
        valid.then_some(Self(value)).ok_or("invalid MG version")
    }
}

/// A fixed readiness outcome code: at most 32 lowercase ASCII letters, digits
/// and `_`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "String")]
pub(crate) struct ReadinessCode(String);

impl TryFrom<String> for ReadinessCode {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let valid = !value.is_empty()
            && value.len() <= MAX_CODE_BYTES
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
        valid.then_some(Self(value)).ok_or("invalid readiness code")
    }
}

/// A finite percentage from 0 to 100.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(try_from = "f64")]
pub(crate) struct Percent(f64);

impl TryFrom<f64> for Percent {
    type Error = &'static str;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        (0.0..=100.0)
            .contains(&value)
            .then_some(Self(value))
            .ok_or("percentage outside 0 to 100")
    }
}

/// A validated report with the Unix millisecond time its record states.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Stated<T> {
    pub(crate) at_ms: i64,
    pub(crate) report: T,
}

impl<T: MgReport> Stated<T> {
    /// Decode `record` when it is a `T` event with a timestamp; `None` for any
    /// other record, and for one whose fields or timestamp do not validate.
    fn decode(record: &serde_json::Value) -> Option<Self> {
        if record["fields"]["message"].as_str() != Some(T::MESSAGE) {
            return None;
        }
        Some(Self {
            at_ms: parse_rfc3339_millis(record["timestamp"].as_str()?)?,
            report: T::deserialize(&record["fields"]).ok()?,
        })
    }
}

impl<T> Stated<T> {
    /// The report's observation time: the stated time, but never later than
    /// reception. The stated time is MG-asserted, so it can only make a report
    /// look older, never fresher than its arrival.
    pub(crate) fn observed_at_ms(&self, received_at_ms: i64) -> i64 {
        self.at_ms.min(received_at_ms)
    }
}

/// The last valid report of each kind in one batch, in journal order.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LatestReports {
    pub(crate) seat_capacity: Option<Stated<SeatCapacity>>,
    pub(crate) seat_readiness: Option<Stated<SeatReadiness>>,
}

impl LatestReports {
    /// Keep `record` if it is a valid report; a later valid record replaces an
    /// earlier one of the same kind. Any other record is ignored.
    pub(crate) fn observe(&mut self, record: &serde_json::Value) {
        if let Some(stated) = Stated::<SeatCapacity>::decode(record) {
            self.seat_capacity = Some(stated);
        } else if let Some(stated) = Stated::<SeatReadiness>::decode(record) {
            self.seat_readiness = Some(stated);
        }
    }
}

/// Parse an RFC 3339 UTC timestamp, as the MG's JSON journal writes it, to
/// Unix milliseconds.
fn parse_rfc3339_millis(timestamp: &str) -> Option<i64> {
    let at = humantime::parse_rfc3339(timestamp).ok()?;
    i64::try_from(at.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()
}

/// Render Unix milliseconds as an RFC 3339 UTC timestamp, for years up to 9999.
pub(crate) fn format_rfc3339_millis(unix_millis: i64) -> Option<String> {
    let millis = u64::try_from(unix_millis).ok()?;
    (millis < YEAR_10000_MILLIS).then(|| {
        humantime::format_rfc3339_millis(UNIX_EPOCH + Duration::from_millis(millis)).to_string()
    })
}

#[cfg(test)]
#[path = "mg_reports_tests.rs"]
mod tests;
