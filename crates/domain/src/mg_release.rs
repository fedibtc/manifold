//! A Manifold Guardian release that MGs install automatically.
//!
//! The setup-payment publication carries it as `mg_release`, next to the
//! `fman_version` it describes. The publication signature only transports it:
//! an MG installs a binary only when a threshold of the release signers that
//! its environment profile pins signed the same release digest.
//!
//! Contract: `specs/SPEC-setup-payment-federations.md`.

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use nostr::PublicKey;
use nostr::secp256k1::Message;
use nostr::secp256k1::schnorr::Signature;
use sha2::{Digest as _, Sha256};

use crate::Url;
use crate::setup_payment_federations::MgVersion;

/// Most per-architecture binaries one release may list.
pub const MG_RELEASE_MAX_BINARIES: usize = 8;

/// Most signatures one release may carry.
pub const MG_RELEASE_MAX_SIGNATURES: usize = 16;

/// Largest binary a release may describe, in bytes.
pub const MG_RELEASE_MAX_BINARY_BYTES: u64 = 1024 * 1024 * 1024;

/// Longest download URL a release may carry, in bytes.
pub const MG_RELEASE_MAX_URL_BYTES: usize = 2048;

/// Domain separator and format version of the signed release digest.
const MG_RELEASE_DIGEST_DOMAIN: &str = "manifold-mg-release/v1";

/// A release: the source it is built from, one static binary per CPU
/// architecture, and the signatures of the release signers.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MgRelease {
    /// Full git commit the binaries are reproducibly built from.
    pub source_commit: GitCommit,

    /// Binaries keyed by CPU architecture, as Rust's
    /// `std::env::consts::ARCH` names it (`x86_64`, `aarch64`).
    pub binaries: BTreeMap<MgReleaseArch, MgReleaseBinary>,

    /// Release-signer signatures over [`MgRelease::digest`].
    pub signatures: Vec<MgReleaseSignature>,
}

/// One downloadable, statically linked `fleet-manager` binary.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MgReleaseBinary {
    /// SHA-256 of the binary file.
    pub sha256: Sha256Hash,

    /// Exact size of the binary file in bytes.
    pub size: u64,

    /// Where to download the binary. The hash, not the URL, is trusted.
    pub url: Url,
}

/// A release signer's BIP-340 signature over the release digest.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MgReleaseSignature {
    /// The signer's Nostr public key.
    pub signer: PublicKey,

    /// Schnorr signature of [`MgRelease::digest`].
    #[serde(with = "signature_hex")]
    pub signature: Signature,
}

impl MgRelease {
    /// The 32-byte digest every release signer signs.
    ///
    /// It binds the environment and the version, so a signature for one
    /// environment or version cannot authorize another. Signatures are not
    /// part of it.
    #[must_use]
    pub fn digest(&self, environment: &str, version: &MgVersion) -> [u8; 32] {
        let mut text = format!(
            "{MG_RELEASE_DIGEST_DOMAIN}\nenvironment={environment}\nversion={version}\ncommit={}\n",
            self.source_commit
        );
        for (arch, binary) in &self.binaries {
            text.push_str(&format!(
                "binary={arch} {} {} {}\n",
                binary.sha256, binary.size, binary.url.0
            ));
        }
        Sha256::digest(text.as_bytes()).into()
    }

    /// The distinct signers whose signature over this release's digest
    /// verifies. Callers compare this set with their pinned signer policy.
    #[must_use]
    pub fn valid_signers(&self, environment: &str, version: &MgVersion) -> BTreeSet<PublicKey> {
        let message = Message::from_digest(self.digest(environment, version));
        self.signatures
            .iter()
            .filter(|signature| {
                signature.signer.xonly().is_ok_and(|key| {
                    nostr::SECP256K1
                        .verify_schnorr(&signature.signature, &message, &key)
                        .is_ok()
                })
            })
            .map(|signature| signature.signer)
            .collect()
    }

    /// Sign this release's digest with one release signer's keys.
    #[must_use]
    pub fn sign(
        &self,
        environment: &str,
        version: &MgVersion,
        keys: &nostr::Keys,
    ) -> MgReleaseSignature {
        let message = Message::from_digest(self.digest(environment, version));
        MgReleaseSignature {
            signer: keys.public_key(),
            signature: keys.sign_schnorr(&message),
        }
    }

    /// The binary for one CPU architecture, if the release has one.
    #[must_use]
    pub fn binary(&self, arch: &str) -> Option<&MgReleaseBinary> {
        self.binaries.get(arch)
    }

    /// Check the bounds and formats that serde does not.
    ///
    /// # Errors
    ///
    /// Returns an error for no binaries, too many binaries or signatures, an
    /// empty or oversized binary, or a download URL that is not a credential-free
    /// HTTP(S) URL within the length limit.
    pub fn validate(&self) -> Result<(), MgReleaseError> {
        if self.binaries.is_empty() || self.binaries.len() > MG_RELEASE_MAX_BINARIES {
            return Err(MgReleaseError::BinaryCount);
        }
        if self.signatures.len() > MG_RELEASE_MAX_SIGNATURES {
            return Err(MgReleaseError::TooManySignatures);
        }
        for binary in self.binaries.values() {
            if binary.size == 0 || binary.size > MG_RELEASE_MAX_BINARY_BYTES {
                return Err(MgReleaseError::BinarySize);
            }
            if binary.url.0.len() > MG_RELEASE_MAX_URL_BYTES {
                return Err(MgReleaseError::InvalidUrl);
            }
            let url = url::Url::parse(&binary.url.0).map_err(|_| MgReleaseError::InvalidUrl)?;
            if !matches!(url.scheme(), "https" | "http")
                || !url.has_host()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.as_str() != binary.url.0
            {
                return Err(MgReleaseError::InvalidUrl);
            }
        }
        Ok(())
    }
}

/// Failure while validating a release.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum MgReleaseError {
    /// The release lists no binaries or more than [`MG_RELEASE_MAX_BINARIES`].
    #[error("release must list between 1 and {MG_RELEASE_MAX_BINARIES} binaries")]
    BinaryCount,
    /// The release carries more than [`MG_RELEASE_MAX_SIGNATURES`] signatures.
    #[error("release carries more than {MG_RELEASE_MAX_SIGNATURES} signatures")]
    TooManySignatures,
    /// A binary is empty or larger than [`MG_RELEASE_MAX_BINARY_BYTES`].
    #[error("release binary size is out of range")]
    BinarySize,
    /// A download URL is not a normalized, credential-free HTTP(S) URL.
    #[error("release binary URL is invalid")]
    InvalidUrl,
}

/// A CPU architecture name, as `std::env::consts::ARCH` spells it.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MgReleaseArch(String);

impl MgReleaseArch {
    /// The architecture this binary was built for.
    #[must_use]
    pub fn current() -> Self {
        Self(std::env::consts::ARCH.to_owned())
    }
}

impl std::borrow::Borrow<str> for MgReleaseArch {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl FromStr for MgReleaseArch {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty()
            || value.len() > 32
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err("architecture must be 1 to 32 lowercase letters, digits, or `_`");
        }
        Ok(Self(value.to_owned()))
    }
}

impl fmt::Display for MgReleaseArch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Fixed-size lowercase-hex value: a SHA-256 hash or a git commit.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HexBytes<const N: usize>([u8; N]);

/// SHA-256 hash, written as 64 lowercase hex digits.
pub type Sha256Hash = HexBytes<32>;

/// Full SHA-1 git commit, written as 40 lowercase hex digits.
pub type GitCommit = HexBytes<20>;

impl<const N: usize> HexBytes<N> {
    /// Wrap raw bytes.
    #[must_use]
    pub fn new(bytes: [u8; N]) -> Self {
        Self(bytes)
    }

    /// The raw bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; N] {
        &self.0
    }
}

impl<const N: usize> FromStr for HexBytes<N> {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 2 * N || value.bytes().any(|byte| byte.is_ascii_uppercase()) {
            return Err("expected lowercase hex of the exact length");
        }
        let mut bytes = [0; N];
        hex::decode_to_slice(value, &mut bytes).map_err(|_| "expected lowercase hex")?;
        Ok(Self(bytes))
    }
}

impl<const N: usize> fmt::Display for HexBytes<N> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&hex::encode(self.0))
    }
}

macro_rules! serde_via_str {
    ($($type:ty),* $(,)?) => {$(
        impl serde::Serialize for $type {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> serde::Deserialize<'de> for $type {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = <String as serde::Deserialize>::deserialize(deserializer)?;
                value.parse().map_err(serde::de::Error::custom)
            }
        }
    )*};
}

serde_via_str!(MgReleaseArch, Sha256Hash, GitCommit);

mod signature_hex {
    use std::str::FromStr as _;

    use nostr::secp256k1::schnorr::Signature;

    pub fn serialize<S: serde::Serializer>(
        signature: &Signature,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_str(signature)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Signature, D::Error> {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Signature::from_str(&value).map_err(serde::de::Error::custom)
    }
}
