//! Canonical derivations from a final Fedimint federation config.
//!
//! Every party that reasons about a federation's identity derives it here. The
//! FMan signs [`federation_config_hash`] into each
//! [`FmanPeerAttestation`](crate::FmanPeerAttestation) it issues, and FLIP
//! re-derives the same hash from the invite-code preview before matching seat
//! bindings against it. A one-byte disagreement between the two silently fails
//! every liquidity request, so neither side may derive its own.
//!
//! Deliberately not derived here: the Bitcoin network. Reading it requires
//! decoding the wallet module config with a module decoder registry, which
//! would pull the module crates into this crate; FLIP's preview owns it.

// `pub(crate)` so the seat-binding tests build their fixtures from the same
// config as the derivations they check.
#[cfg(test)]
pub(crate) mod tests;

use std::collections::BTreeMap;

use fedimint_core::config::ClientConfig;
use fedimint_core::encoding::Encodable;
use fedimint_core::{NumPeers, PeerId as FedimintPeerId};
use sha2::{Digest, Sha256};

use crate::{FederationId, GuardianIdentity, HashBytes, PeerId};

/// Domain separator for the canonical federation config hash.
pub const FEDERATION_CONFIG_HASH_DOMAIN_SEPARATOR: &[u8] = b"fedi-federation-config-hash/v1\0";

/// Canonical hash of a final Fedimint federation config.
///
/// The hash covers the consensus encoding of the whole client config with
/// `global.meta` cleared, under
/// [`FEDERATION_CONFIG_HASH_DOMAIN_SEPARATOR`]. Everything that fixes the
/// federation's identity is therefore bound: API endpoints, broadcast public
/// keys, the core consensus version, and every module config.
///
/// Consensus metadata is excluded because it is the federation's mutable,
/// self-declared surface. The FI publishes `fedi:fman_seat_bindings` into it
/// *after* the guardians have signed their attestations, so a hash that covered
/// metadata would invalidate every attestation the moment the directory it
/// belongs to is published. Fedimint 0.11 keeps post-DKG metadata writes in the
/// `meta` module's consensus state rather than in `global.meta`, so clearing
/// the map is belt-and-braces today; it makes the invariant structural instead
/// of contingent on where a metadata write happens to land.
///
/// The consensus encoding of a config is decoder-independent: an undecoded
/// module config (`DynRawFallback::Raw`) encodes to the same bytes as its
/// decoded form, so a verifier holding no module crates derives the same hash
/// as the guardian that produced the config.
#[must_use]
pub fn federation_config_hash(config: &ClientConfig) -> HashBytes {
    let mut hashable = config.clone();
    hashable.global.meta = BTreeMap::new();

    let digest = Sha256::new()
        .chain_update(FEDERATION_CONFIG_HASH_DOMAIN_SEPARATOR)
        .chain_update(hashable.consensus_encode_to_vec())
        .finalize();

    HashBytes(digest.to_vec())
}

/// Canonical protocol federation id for a final Fedimint federation config.
///
/// This is Fedimint's own federation id in its lowercase-hex `Display` form,
/// which is also what [`fedimint_core::invite_code::InviteCode::federation_id`]
/// yields, so a cheap parse of an invite code and a full config download agree.
#[must_use]
pub fn federation_id(config: &ClientConfig) -> FederationId {
    FederationId(config.calculate_federation_id().to_string())
}

/// Consensus-majority threshold for a federation of `total_peers` guardians.
///
/// This is Fedimint's own [`NumPeers::threshold`] (`n - (n - 1) / 3`), guarded
/// against the degenerate empty peer set that would underflow it and against a
/// peer count that does not fit the protocol's threshold width.
#[must_use]
pub fn consensus_threshold(total_peers: usize) -> Option<u32> {
    if total_peers == 0 {
        return None;
    }

    u32::try_from(NumPeers::from(total_peers).threshold()).ok()
}

/// Protocol peer id for a Fedimint peer id.
///
/// The canonical form is the decimal `Display` output; alternate spellings
/// (leading zeros, a `+` sign) are rejected by [`parse_protocol_peer_id`].
#[must_use]
pub fn protocol_peer_id(peer: FedimintPeerId) -> PeerId {
    PeerId(peer.to_string())
}

/// Parse a protocol peer id back into a Fedimint peer id.
///
/// Returns `None` unless the value is the exact canonical decimal spelling, so
/// that `"0"` parses while `"00"` and `"+0"` do not.
#[must_use]
pub fn parse_protocol_peer_id(value: &PeerId) -> Option<FedimintPeerId> {
    let parsed = value.0.parse::<FedimintPeerId>().ok()?;
    (parsed.to_string() == value.0).then_some(parsed)
}

/// One guardian seat from a final federation config.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FederationSeat {
    /// Fedimint peer id operating this seat.
    pub peer_id: PeerId,

    /// Guardian consensus identity for this seat.
    pub guardian_identity: GuardianIdentity,
}

/// Authoritative federation facts derived from a final federation config.
///
/// Both the FMan issuing attestations and the verifier checking them build this
/// from the same config, so a mismatch anywhere in it is a real disagreement
/// about which federation is being talked about.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FederationSeats {
    /// Canonical federation id.
    federation_id: FederationId,

    /// Canonical federation config hash.
    federation_config_hash: HashBytes,

    /// Consensus-majority threshold for the peer set.
    consensus_threshold: u32,

    /// Guardian seats in ascending peer-id order.
    seats: Vec<FederationSeat>,
}

impl FederationSeats {
    /// Assemble federation facts a verifier obtained other than from a
    /// `ClientConfig`.
    ///
    /// [`federation_seats`] is the derivation of record. This exists for
    /// verifiers that already hold the facts in another shape — FLIP's
    /// invite-code preview, or a test fixture standing in for one — and must
    /// check seat bindings against them. Callers own the correspondence
    /// between what they pass and the config it came from.
    #[must_use]
    pub fn from_parts(
        federation_id: FederationId,
        federation_config_hash: HashBytes,
        consensus_threshold: u32,
        seats: Vec<FederationSeat>,
    ) -> Self {
        Self {
            federation_id,
            federation_config_hash,
            consensus_threshold,
            seats,
        }
    }

    /// Return the canonical federation id.
    #[must_use]
    pub fn federation_id(&self) -> &FederationId {
        &self.federation_id
    }

    /// Return the canonical federation config hash.
    #[must_use]
    pub fn federation_config_hash(&self) -> &HashBytes {
        &self.federation_config_hash
    }

    /// Return the consensus-majority threshold for this peer set.
    #[must_use]
    pub fn consensus_threshold(&self) -> u32 {
        self.consensus_threshold
    }

    /// Return the guardian seats in ascending peer-id order.
    #[must_use]
    pub fn seats(&self) -> &[FederationSeat] {
        &self.seats
    }

    /// Look up one guardian seat by peer id.
    #[must_use]
    pub fn seat(&self, peer_id: &PeerId) -> Option<&FederationSeat> {
        self.seats.iter().find(|seat| &seat.peer_id == peer_id)
    }
}

/// Error returned when deriving federation facts from a final config.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FederationConfigError {
    /// The config declares no guardian seats.
    NoPeers,

    /// The peer count has no representable consensus threshold.
    ThresholdUnavailable,

    /// The config carries no broadcast public keys, so guardian consensus
    /// identities cannot be derived. Fedimint configs older than 0.4 are the
    /// only ones that look like this.
    MissingBroadcastPublicKeys,

    /// A config peer has no broadcast public key.
    MissingGuardianIdentity(PeerId),
}

impl core::fmt::Display for FederationConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoPeers => f.write_str("federation config declares no guardian seats"),
            Self::ThresholdUnavailable => {
                f.write_str("federation config peer count has no representable threshold")
            }
            Self::MissingBroadcastPublicKeys => {
                f.write_str("federation config carries no broadcast public keys")
            }
            Self::MissingGuardianIdentity(peer_id) => {
                write!(
                    f,
                    "federation config peer {} has no broadcast public key",
                    peer_id.0
                )
            }
        }
    }
}

impl std::error::Error for FederationConfigError {}

/// Derive the authoritative federation facts from a final config.
///
/// The guardian consensus identity is the peer's broadcast public key in
/// lowercase-hex compressed secp256k1 form. That key, not the peer's
/// self-chosen display name, is what signs the federation's consensus session
/// outcomes, so it is the identity an attestation is worth binding to.
///
/// # Errors
///
/// Returns an error if the config has no peers, has no representable consensus
/// threshold, or predates broadcast public keys.
pub fn federation_seats(config: &ClientConfig) -> Result<FederationSeats, FederationConfigError> {
    if config.global.api_endpoints.is_empty() {
        return Err(FederationConfigError::NoPeers);
    }
    let broadcast_public_keys = config
        .global
        .broadcast_public_keys
        .as_ref()
        .ok_or(FederationConfigError::MissingBroadcastPublicKeys)?;

    // `api_endpoints` is a `BTreeMap<PeerId, _>`, so this walks the peer set in
    // ascending peer-id order — the same order the seat-binding container is
    // canonicalized in.
    let seats = config
        .global
        .api_endpoints
        .keys()
        .map(|peer| {
            let peer_id = protocol_peer_id(*peer);
            let guardian_key = broadcast_public_keys
                .get(peer)
                .ok_or_else(|| FederationConfigError::MissingGuardianIdentity(peer_id.clone()))?;

            Ok(FederationSeat {
                peer_id,
                guardian_identity: GuardianIdentity(guardian_key.to_string()),
            })
        })
        .collect::<Result<Vec<_>, FederationConfigError>>()?;

    let consensus_threshold =
        consensus_threshold(seats.len()).ok_or(FederationConfigError::ThresholdUnavailable)?;

    Ok(FederationSeats {
        federation_id: federation_id(config),
        federation_config_hash: federation_config_hash(config),
        consensus_threshold,
        seats,
    })
}
