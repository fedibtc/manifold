//! Release and API version messages.

use crate::MgVersion;

/// The version of the MG API, counted like Fedimint's API minor version: it
/// goes up by one each time the API changes, and a client checks a feature
/// with `FEATURE_ADDED_IN <= api_version`. An MG from before `GetVersion`
/// has no API version and is below every feature that names one.
#[derive(
    serde::Deserialize, serde::Serialize, Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd,
)]
#[serde(transparent)]
pub struct MgApiVersion(pub u32);

/// The first API version: `GetVersion` itself, and the guardian transfer
/// whose `TransferSeat` carries the destination's admission proof
/// ([`crate::TransferSeatCall`]). MG 0.1.9 spoke an earlier, proofless
/// transfer and had no `GetVersion`.
pub const GUARDIAN_TRANSFER_V2_API_VERSION: MgApiVersion = MgApiVersion(1);

/// The API version this build serves.
pub const MG_API_VERSION: MgApiVersion = GUARDIAN_TRANSFER_V2_API_VERSION;

/// Request for this MG's release and API version.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct GetVersionRequest;

/// This MG's release and API version.
///
/// The answer describes the build, not policy: an MG at an API version with
/// guardian transfer still starts a transfer only while its setup-payment
/// policy enables transfer.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct GetVersionResponse {
    /// The MG release.
    pub mg_version: MgVersion,

    /// The MG API version.
    pub api_version: MgApiVersion,
}

impl GetVersionResponse {
    /// Whether this MG serves a feature added in API version `added_in`.
    #[must_use]
    pub fn supports(&self, added_in: MgApiVersion) -> bool {
        added_in <= self.api_version
    }
}

#[cfg(test)]
mod tests;
