//! Domain identity for one native Fedimint payout operation.

use std::fmt;
use std::str::FromStr;

use bitcoin_hashes::{Hash as _, sha256};
use fedimint_core::core::OperationId;

use crate::payout_job::PayoutRequestId;

/// An exact native Fedimint payout operation identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Serialize)]
#[serde(transparent)]
pub struct PayoutOperationId(String);

impl PayoutOperationId {
    /// Parse the canonical 32-byte lowercase hexadecimal operation identifier.
    pub fn parse(encoded: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            encoded.len() == 64
                && encoded
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "invalid payout operation id"
        );
        Ok(Self(encoded.to_owned()))
    }

    /// Return the canonical hexadecimal identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The operation an ecash payout's spend is recorded under: a function of
    /// its request id alone, so a retry finds the spend instead of making
    /// another.
    pub(crate) fn ecash(request_id: &PayoutRequestId) -> Self {
        let mut engine = sha256::Hash::engine();
        bitcoin_hashes::HashEngine::input(&mut engine, b"fman-payout-ecash\0");
        bitcoin_hashes::HashEngine::input(&mut engine, request_id.as_str().as_bytes());
        Self::from_native(OperationId(
            sha256::Hash::from_engine(engine).to_byte_array(),
        ))
    }

    pub(crate) fn from_native(id: OperationId) -> Self {
        Self(id.fmt_full().to_string())
    }

    pub(crate) fn to_native(&self) -> OperationId {
        self.0
            .parse()
            .expect("PayoutOperationId validates Fedimint's native encoding")
    }
}

impl fmt::Display for PayoutOperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for PayoutOperationId {
    type Err = anyhow::Error;

    fn from_str(encoded: &str) -> Result<Self, Self::Err> {
        Self::parse(encoded)
    }
}

impl<'de> serde::Deserialize<'de> for PayoutOperationId {
    fn deserialize<Deserializer>(deserializer: Deserializer) -> Result<Self, Deserializer::Error>
    where
        Deserializer: serde::Deserializer<'de>,
    {
        let encoded = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::parse(&encoded).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests;
