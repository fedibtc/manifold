//! The values describing one provider deposit into a target stability pool: a
//! locally generated operation id, the immutable request submitted under it,
//! and the versioned metadata persisted with the Fedimint operation.

use std::fmt;
use std::str::FromStr;

use fedi_decentralized_service_liquidity_manager::Sats;
use fedimint_core::core::OperationId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A locally generated identifier for one stability-pool deposit submission.
///
/// The private field prevents request data, allocation item identifiers, and
/// arbitrary hashes from becoming operation identifiers. Deserialization is
/// reserved for reloading the value that FLIP already persisted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StabilityDepositOperationId(OperationId);

impl StabilityDepositOperationId {
    /// Generates an unpredictable operation identifier locally.
    pub(crate) fn generate() -> Self {
        Self(OperationId::new_random())
    }

    /// Returns the Fedimint identifier at the concrete backend boundary.
    pub(crate) fn as_fedimint(self) -> OperationId {
        self.0
    }

    /// Parses an operation identifier supplied by the operator recovery API.
    pub(crate) fn parse(encoded: &str) -> anyhow::Result<Self> {
        Ok(Self(OperationId::from_str(encoded)?))
    }
}

impl fmt::Display for StabilityDepositOperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt_full().fmt(formatter)
    }
}

impl Serialize for StabilityDepositOperationId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for StabilityDepositOperationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        OperationId::from_str(&encoded)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

/// One caller-owned operation ID and its immutable provider-deposit request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StabilityDepositSubmission {
    /// Locally generated operation identifier.
    operation_id: StabilityDepositOperationId,
    /// Exact amount submitted to the stability-pool module.
    amount: Sats,
    /// Exact minimum fee rate submitted to the stability-pool module.
    min_fee_rate_ppb: u64,
}

impl StabilityDepositSubmission {
    /// Generates a new candidate with the supplied immutable parameters.
    pub(crate) fn generate(amount: Sats, min_fee_rate_ppb: u64) -> Self {
        Self {
            operation_id: StabilityDepositOperationId::generate(),
            amount,
            min_fee_rate_ppb,
        }
    }

    /// Rehydrates the operation ID and request loaded from durable storage.
    pub(crate) fn rehydrate(
        operation_id: StabilityDepositOperationId,
        amount: Sats,
        min_fee_rate_ppb: u64,
    ) -> Self {
        Self {
            operation_id,
            amount,
            min_fee_rate_ppb,
        }
    }

    pub(crate) fn operation_id(self) -> StabilityDepositOperationId {
        self.operation_id
    }

    pub(crate) fn amount(self) -> Sats {
        self.amount
    }

    pub(crate) fn min_fee_rate_ppb(self) -> u64 {
        self.min_fee_rate_ppb
    }
}

/// Versioned commitment stored with one caller-owned Fedimint operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct StabilityDepositMetadata {
    /// Metadata schema version.
    version: u8,
    /// Stability-pool account role receiving the deposit.
    role: String,
    /// FLIP allocation item that owns the operation.
    item_id: String,
    /// Exact provider-deposit amount in sats.
    amount_sats: u64,
    /// Exact minimum provider fee rate in parts per billion.
    min_fee_rate_ppb: u64,
}

impl StabilityDepositMetadata {
    const VERSION: u8 = 1;

    /// Commits one immutable FLIP submission to its allocation item.
    pub(crate) fn new(item_id: &str, submission: StabilityDepositSubmission) -> Self {
        Self {
            version: Self::VERSION,
            role: "provider".to_owned(),
            item_id: item_id.to_owned(),
            amount_sats: submission.amount().0,
            min_fee_rate_ppb: submission.min_fee_rate_ppb(),
        }
    }

    /// Checks that metadata names exactly this immutable request.
    pub(crate) fn matches(&self, item_id: &str, submission: StabilityDepositSubmission) -> bool {
        self.version == Self::VERSION
            && self.role == "provider"
            && self.item_id == item_id
            && self.amount_sats == submission.amount().0
            && self.min_fee_rate_ppb == submission.min_fee_rate_ppb()
    }
}

#[cfg(test)]
#[path = "../tests/stability_deposit.rs"]
mod tests;
