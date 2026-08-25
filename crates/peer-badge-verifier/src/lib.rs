//! Complete shared PeerBadge-envelope verification for relying components.
//!
//! The governing contract is
//! `specs/SPEC-peer-badge-verifier.md`.

mod verifier;

pub use verifier::{
    PeerBadgeVerificationError, PeerBadgeVerifier, PeerBadgeVerifierConfigError,
    PeerBadgeVerifierProvenance, VerifiedPeerBadge,
};
