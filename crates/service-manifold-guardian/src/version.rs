//! Release and protocol version messages.

use crate::MgVersion;

/// The guardian transfer protocol version whose `TransferSeat` carries the
/// destination's admission proof ([`crate::TransferSeatCall`]). MG 0.1.9
/// spoke an earlier, proofless shape and no version endpoint.
pub const GUARDIAN_TRANSFER_PROTOCOL_V2: u32 = 2;

/// Request for this MG's release and the protocol versions it speaks.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct GetVersionRequest;

/// This MG's release and the protocol versions it speaks.
///
/// The answer describes the build, not policy: an MG that speaks a transfer
/// protocol still starts a transfer only while its setup-payment policy
/// enables transfer.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct GetVersionResponse {
    /// The MG release.
    pub mg_version: MgVersion,

    /// The protocol versions this MG speaks.
    pub protocols: SupportedProtocols,
}

/// The versions of each protocol an MG speaks. A reader ignores protocols it
/// does not know, so a later release adds one as a new field.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct SupportedProtocols {
    /// Guardian transfer protocol versions, such as
    /// [`GUARDIAN_TRANSFER_PROTOCOL_V2`].
    pub guardian_transfer: Vec<u32>,
}

impl GetVersionResponse {
    /// Whether this MG speaks guardian transfer protocol `version`.
    #[must_use]
    pub fn supports_guardian_transfer(&self, version: u32) -> bool {
        self.protocols.guardian_transfer.contains(&version)
    }
}

#[cfg(test)]
mod tests;
