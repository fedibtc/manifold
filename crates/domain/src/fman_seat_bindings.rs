//! The `fedi:fman_seat_bindings` consensus-metadata container.
//!
//! The container answers "which FMan identity operates each guardian seat of
//! this federation" for anyone holding only an invite code. It carries binding
//! claims only: badges, holder authorizations, and revocations are resolved
//! separately from Nostr and stay untrusted until verified.
//!
//! Validation splits in two, and both the FMan `ProposeFormationMeta` handler and every
//! reader run both halves:
//!
//! - [`FmanSeatBindings::parse_canonical`] enforces the value's own shape —
//!   canonical JSON, version, ordering, deduplication, and size caps. It needs
//!   no federation context.
//! - [`FmanSeatBindings::verify_for_federation`] enforces the binding contract
//!   against a final config: every signature verifies, and the bindings match
//!   the config's guardian seats exactly, with no missing, extra, or
//!   conflicting entry.

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use serde::de::Error as _;
use stability_pool_common::{Account, AccountType};

use crate::federation_config::{FederationSeats, parse_protocol_peer_id};
use crate::fman_peer_attestation::FmanPeerAttestation;
use crate::{GuardianIdentity, PeerId, ProtocolV1, Pubkey};

/// Fedimint consensus metadata key holding the FMan seat-binding directory.
pub const FMAN_SEAT_BINDINGS_META_FIELD_KEY: &str = "fedi:fman_seat_bindings";

/// Maximum seat bindings accepted in one directory value (provisional).
pub const FMAN_SEAT_BINDINGS_MAX_COUNT: usize = 64;

/// Maximum canonical JSON value length accepted for `fedi:fman_seat_bindings`
/// (provisional).
pub const FMAN_SEAT_BINDINGS_MAX_VALUE_BYTES: usize = 65_536;

/// Canonical Fedimint metadata directory value binding guardian seats to FMans.
///
/// Construction and parsing guarantee the structural invariants; nothing here
/// is trusted until [`FmanSeatBindings::verify_for_federation`] has checked it
/// against the federation's final config.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct FmanSeatBindings {
    /// Protocol version for this metadata value.
    version: ProtocolV1,

    /// Seat bindings in ascending peer-id order, one per peer id.
    seat_bindings: Vec<FmanPeerAttestation>,
}

impl FmanSeatBindings {
    /// Build a validated directory value from arbitrary seat bindings.
    ///
    /// Bindings are sorted into ascending peer-id order. Two bindings for the
    /// same peer id are a conflict, not a duplicate to collapse, so they are
    /// rejected rather than deduplicated.
    ///
    /// # Errors
    ///
    /// Returns an error if the binding list is empty, carries a non-canonical
    /// peer id, claims one peer id twice, or exceeds the count or canonical
    /// value size caps.
    pub fn new(
        seat_bindings: impl IntoIterator<Item = FmanPeerAttestation>,
    ) -> Result<Self, FmanSeatBindingsError> {
        let seat_bindings = seat_bindings.into_iter().collect::<Vec<_>>();
        if seat_bindings.is_empty() {
            return Err(FmanSeatBindingsError::Empty);
        }
        if seat_bindings.len() > FMAN_SEAT_BINDINGS_MAX_COUNT {
            return Err(FmanSeatBindingsError::TooManySeatBindings);
        }

        // Sort on the parsed peer id, not its spelling: lexicographic ordering
        // would put peer 10 ahead of peer 2 and disagree with the config's own
        // `BTreeMap<PeerId, _>` order.
        let mut keyed = Vec::with_capacity(seat_bindings.len());
        for binding in seat_bindings {
            let peer_id = &binding.attestation.peer_id;
            let parsed =
                parse_protocol_peer_id(peer_id).ok_or(FmanSeatBindingsError::NonCanonicalPeerId)?;
            keyed.push((parsed, binding));
        }
        keyed.sort_by_key(|(peer_id, _)| *peer_id);
        for window in keyed.windows(2) {
            if window[0].0 == window[1].0 {
                return Err(FmanSeatBindingsError::DuplicatePeerId(
                    window[0].1.attestation.peer_id.clone(),
                ));
            }
        }

        let bindings = Self {
            version: ProtocolV1,
            seat_bindings: keyed.into_iter().map(|(_, binding)| binding).collect(),
        };
        if bindings.canonical_bytes()?.len() > FMAN_SEAT_BINDINGS_MAX_VALUE_BYTES {
            return Err(FmanSeatBindingsError::ValueTooLarge);
        }

        Ok(bindings)
    }

    /// Return the protocol version for this metadata value.
    #[must_use]
    pub fn version(&self) -> ProtocolV1 {
        self.version
    }

    /// Return the seat bindings in ascending peer-id order.
    #[must_use]
    pub fn seat_bindings(&self) -> &[FmanPeerAttestation] {
        &self.seat_bindings
    }

    /// Parse and validate a canonical metadata value.
    ///
    /// The value must be exactly the canonical serialization of what it
    /// decodes to: equivalent JSON that differs in key order, whitespace, or
    /// binding order is rejected, so every reader of the directory sees the
    /// same bytes the writers agreed on.
    ///
    /// # Errors
    ///
    /// Returns an error if the value is oversized, is not valid JSON, is not
    /// exactly canonical, or fails the structural invariants.
    pub fn parse_canonical(value: &str) -> Result<Self, FmanSeatBindingsError> {
        if value.len() > FMAN_SEAT_BINDINGS_MAX_VALUE_BYTES {
            return Err(FmanSeatBindingsError::ValueTooLarge);
        }
        let parsed: RawFmanSeatBindings =
            serde_json::from_str(value).map_err(|_| FmanSeatBindingsError::MalformedJson)?;
        let normalized = Self::new(parsed.seat_bindings)?;
        if normalized.canonical_string()? != value {
            return Err(FmanSeatBindingsError::NonCanonical);
        }

        Ok(normalized)
    }

    /// Serialize this value as canonical JSON bytes suitable for Fedimint
    /// metadata.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, FmanSeatBindingsError> {
        serde_json_canonicalizer::to_vec(self)
            .map_err(|_| FmanSeatBindingsError::CanonicalizationFailed)
    }

    /// Serialize this value as a canonical JSON string.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails or produces
    /// non-UTF-8 bytes.
    pub fn canonical_string(&self) -> Result<String, FmanSeatBindingsError> {
        String::from_utf8(self.canonical_bytes()?)
            .map_err(|_| FmanSeatBindingsError::CanonicalizationFailed)
    }

    /// Verify every binding against a federation's final config.
    ///
    /// Each binding's signature must verify under its own claimed
    /// `fman_pubkey`, and must name this federation, this config hash, and one
    /// of this config's guardian seats with that seat's guardian identity.
    /// Bindings must cover the config's seats exactly: a seat with no binding
    /// and a binding for a peer that is not a seat are both fatal.
    ///
    /// The returned bindings are in ascending peer-id order. One FMan may
    /// legitimately appear on several seats, so callers applying policy over
    /// operating identities must reduce to distinct `fman_pubkey` values.
    ///
    /// # Errors
    ///
    /// Returns an error on the first binding that fails signature
    /// verification or does not match the config, and on any seat coverage
    /// mismatch.
    pub fn verify_for_federation(
        &self,
        federation: &FederationSeats,
    ) -> Result<Vec<VerifiedSeatBinding>, FmanSeatBindingsError> {
        let mut verified = Vec::with_capacity(self.seat_bindings.len());
        let mut guardian_fee_accounts = BTreeSet::new();
        for binding in &self.seat_bindings {
            let statement = binding
                .verify()
                .map_err(|_| FmanSeatBindingsError::InvalidSeatBindingSignature)?;
            if &statement.federation_id != federation.federation_id() {
                return Err(FmanSeatBindingsError::FederationMismatch(statement.peer_id));
            }
            if &statement.federation_config_hash != federation.federation_config_hash() {
                return Err(FmanSeatBindingsError::ConfigHashMismatch(statement.peer_id));
            }
            let Some(seat) = federation.seat(&statement.peer_id) else {
                return Err(FmanSeatBindingsError::UnknownPeerId(statement.peer_id));
            };
            if statement.guardian_identity != seat.guardian_identity {
                return Err(FmanSeatBindingsError::GuardianIdentityMismatch(
                    statement.peer_id,
                ));
            }
            if statement.guardian_fee_account.acc_type() != AccountType::BtcDepositor
                || statement.guardian_fee_account.as_single().is_none()
            {
                return Err(FmanSeatBindingsError::InvalidGuardianFeeAccount(
                    statement.peer_id,
                ));
            }
            if !guardian_fee_accounts.insert(statement.guardian_fee_account.id()) {
                return Err(FmanSeatBindingsError::DuplicateGuardianFeeAccount(
                    statement.peer_id,
                ));
            }
            verified.push(VerifiedSeatBinding {
                attestation: binding.clone(),
                peer_id: statement.peer_id,
                guardian_identity: statement.guardian_identity,
                fman_pubkey: statement.fman_pubkey,
                guardian_fee_account: statement.guardian_fee_account,
            });
        }

        // Bindings for peers that are not seats already failed above, so the
        // only coverage failure left is a seat nothing bound.
        let bound = verified
            .iter()
            .map(|binding| &binding.peer_id)
            .collect::<BTreeSet<_>>();
        if let Some(missing) = federation
            .seats()
            .iter()
            .find(|seat| !bound.contains(&seat.peer_id))
        {
            return Err(FmanSeatBindingsError::UnboundPeerId(
                missing.peer_id.clone(),
            ));
        }

        Ok(verified)
    }
}

impl<'de> serde::Deserialize<'de> for FmanSeatBindings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawFmanSeatBindings::deserialize(deserializer)?;
        let _version = raw.version;
        let raw_order = peer_ids(&raw.seat_bindings);
        let bindings = Self::new(raw.seat_bindings).map_err(D::Error::custom)?;
        if raw_order != peer_ids(&bindings.seat_bindings) {
            return Err(D::Error::custom(FmanSeatBindingsError::NonCanonical));
        }

        Ok(bindings)
    }
}

/// Raw serde shape for [`FmanSeatBindings`] before invariant validation.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFmanSeatBindings {
    /// Protocol version for this metadata value.
    version: ProtocolV1,

    /// Raw seat bindings.
    seat_bindings: Vec<FmanPeerAttestation>,
}

/// One seat binding that verified against a federation's final config.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedSeatBinding {
    /// Original FMan-signed attestation carried by consensus metadata.
    pub attestation: FmanPeerAttestation,

    /// Fedimint peer id this binding covers.
    pub peer_id: PeerId,

    /// Guardian consensus identity of that peer in the final config.
    pub guardian_identity: GuardianIdentity,

    /// FMan identity that signed the binding and therefore operates the seat.
    pub fman_pubkey: Pubkey,

    /// FMan-signed, consensus-bound fee account for this guardian seat.
    pub guardian_fee_account: Account,
}

/// Error returned when validating the `fedi:fman_seat_bindings` metadata value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FmanSeatBindingsError {
    /// The directory value carries no seat bindings.
    Empty,

    /// The directory value carries more seat bindings than the cap allows.
    TooManySeatBindings,

    /// The canonical directory value exceeds the configured size limit.
    ValueTooLarge,

    /// The directory value is not valid JSON.
    MalformedJson,

    /// The directory JSON is valid but not exactly canonical.
    NonCanonical,

    /// Canonical JSON serialization failed.
    CanonicalizationFailed,

    /// A seat binding carries a peer id that is not canonical decimal.
    NonCanonicalPeerId,

    /// More than one seat binding claims the same peer id.
    DuplicatePeerId(PeerId),

    /// A seat binding's FMan signature does not verify.
    InvalidSeatBindingSignature,

    /// A seat binding names a different federation.
    FederationMismatch(PeerId),

    /// A seat binding names a different federation config hash.
    ConfigHashMismatch(PeerId),

    /// A seat binding names a peer that is not a guardian seat of this config.
    UnknownPeerId(PeerId),

    /// A seat binding names a different guardian identity than the config's.
    GuardianIdentityMismatch(PeerId),

    /// A binding's fee account is not a single-signature BTC depositor.
    InvalidGuardianFeeAccount(PeerId),

    /// Two guardian seats claim the same fee account.
    DuplicateGuardianFeeAccount(PeerId),

    /// A guardian seat of this config has no seat binding.
    UnboundPeerId(PeerId),
}

impl core::fmt::Display for FmanSeatBindingsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => f.write_str("FMan seat bindings value is empty"),
            Self::TooManySeatBindings => {
                f.write_str("FMan seat bindings value has too many bindings")
            }
            Self::ValueTooLarge => f.write_str("FMan seat bindings value is too large"),
            Self::MalformedJson => f.write_str("FMan seat bindings JSON is malformed"),
            Self::NonCanonical => f.write_str("FMan seat bindings value is not canonical"),
            Self::CanonicalizationFailed => {
                f.write_str("FMan seat bindings canonicalization failed")
            }
            Self::NonCanonicalPeerId => f.write_str("FMan seat binding peer id is not canonical"),
            Self::DuplicatePeerId(peer_id) => {
                write!(f, "duplicate FMan seat binding for peer {}", peer_id.0)
            }
            Self::InvalidSeatBindingSignature => f.write_str("invalid FMan seat binding signature"),
            Self::FederationMismatch(peer_id) => write!(
                f,
                "FMan seat binding for peer {} names another federation",
                peer_id.0
            ),
            Self::ConfigHashMismatch(peer_id) => write!(
                f,
                "FMan seat binding for peer {} names another federation config",
                peer_id.0
            ),
            Self::UnknownPeerId(peer_id) => write!(
                f,
                "FMan seat binding names peer {}, which is not a guardian seat",
                peer_id.0
            ),
            Self::GuardianIdentityMismatch(peer_id) => write!(
                f,
                "FMan seat binding for peer {} names another guardian identity",
                peer_id.0
            ),
            Self::InvalidGuardianFeeAccount(peer_id) => write!(
                f,
                "FMan seat binding for peer {} has an invalid guardian-fee account",
                peer_id.0
            ),
            Self::DuplicateGuardianFeeAccount(peer_id) => write!(
                f,
                "FMan seat binding for peer {} repeats a guardian-fee account",
                peer_id.0
            ),
            Self::UnboundPeerId(peer_id) => {
                write!(f, "guardian seat {} has no FMan seat binding", peer_id.0)
            }
        }
    }
}

impl std::error::Error for FmanSeatBindingsError {}

/// Peer ids claimed by a binding list, in its own order.
fn peer_ids(seat_bindings: &[FmanPeerAttestation]) -> Vec<PeerId> {
    seat_bindings
        .iter()
        .map(|binding| binding.attestation.peer_id.clone())
        .collect()
}
