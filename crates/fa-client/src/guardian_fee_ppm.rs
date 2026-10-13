//! Guardian fee rates expressed in parts per million.

use std::fmt;

/// A guardian fee rate in the inclusive `0..=1_000_000` parts-per-million domain.
///
/// Serde encodes and decodes this type as an unwrapped numeric value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct GuardianFeePpm(u32);

/// Error returned when a guardian fee lies outside the parts-per-million domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidGuardianFeePpm;

impl GuardianFeePpm {
    /// A zero guardian fee rate.
    pub const ZERO: Self = Self(0);

    /// Default initial and post-formation rate for a Manifold federation:
    /// 0.5%, represented exactly as 5,000 ppm.
    pub const MANIFOLD_DEFAULT: Self = Self(5_000);

    /// Return the fee rate as parts per million.
    pub const fn value(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for GuardianFeePpm {
    type Error = InvalidGuardianFeePpm;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        const ONE_MILLION: u32 = 1_000_000;

        if value <= ONE_MILLION {
            Ok(Self(value))
        } else {
            Err(InvalidGuardianFeePpm)
        }
    }
}

impl From<GuardianFeePpm> for u32 {
    fn from(value: GuardianFeePpm) -> Self {
        value.value()
    }
}

impl fmt::Display for InvalidGuardianFeePpm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("guardian fee ppm must not exceed 1000000")
    }
}

impl std::error::Error for InvalidGuardianFeePpm {}

#[cfg(test)]
mod tests;
