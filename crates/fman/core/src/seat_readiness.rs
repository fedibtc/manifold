//! The daemon's latest verdict on whether a new seat would be served.
//!
//! The daemon probes its prerequisites and hands each report to
//! [`crate::fleet::Fleet::set_seat_readiness`], which gates admission on it.
//! Only fixed codes cross this boundary: the report is shared as telemetry and
//! shown to the operator.

/// One prerequisite's outcome; never a remote value or error text.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessOutcome {
    Pass,
    /// The check does not apply to this deployment.
    NotApplicable,
    RelayDisconnected,
    DiscoveryRecordMissing,
    BitcoinUnavailable,
    BitcoinWrongNetwork,
    BitcoinSyncing,
    BitcoinNoFeeRate,
}

impl ReadinessOutcome {
    pub fn passed(self) -> bool {
        matches!(self, Self::Pass | Self::NotApplicable)
    }

    /// The serialized code, for tracing fields.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::NotApplicable => "not_applicable",
            Self::RelayDisconnected => "relay_disconnected",
            Self::DiscoveryRecordMissing => "discovery_record_missing",
            Self::BitcoinUnavailable => "bitcoin_unavailable",
            Self::BitcoinWrongNetwork => "bitcoin_wrong_network",
            Self::BitcoinSyncing => "bitcoin_syncing",
            Self::BitcoinNoFeeRate => "bitcoin_no_fee_rate",
        }
    }
}

/// One completed readiness run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, ts_rs::TS)]
pub struct ReadinessReport {
    /// Unix time the run completed.
    #[ts(type = "number")]
    pub checked_at_ms: u64,
    pub relay: ReadinessOutcome,
    pub discovery: ReadinessOutcome,
    pub bitcoin: ReadinessOutcome,
}

impl ReadinessReport {
    pub fn ready(&self) -> bool {
        self.relay.passed() && self.discovery.passed() && self.bitcoin.passed()
    }
}
