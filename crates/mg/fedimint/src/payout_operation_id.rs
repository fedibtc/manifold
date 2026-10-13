//! Domain identity for one native Fedimint payout operation.

use std::fmt;
use std::str::FromStr;

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
