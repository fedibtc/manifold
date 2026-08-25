use core::fmt;

use petname::Petnames;
use sha2::{Digest as _, Sha256};

/// Domain separating FMan names from every other use of the identity key.
const FMAN_NAME_DOMAIN: &[u8] = b"fman-name/v1\0";

/// Deterministic two-word display name for one FMan public identity.
///
/// This is a human-readable fingerprint, not an identity: names can collide,
/// and callers must continue to use the authenticated FMan public key for
/// identity, trust, and deduplication.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FmanName(String);

impl FmanName {
    /// Derive the name from the validated Nostr public key that identifies the FMan.
    ///
    /// Hash `fman-name/v1\0 || canonical public-key bytes` with SHA-256. Interpret
    /// the first two eight-byte digest chunks as big-endian integers, then select an
    /// adjective and noun modulo the lengths of `petname` 3.1.0's medium English
    /// lists. The resulting lowercase-ASCII `adjective-noun` string is stable; the
    /// domain separator, byte order, dependency version, list choice, and golden
    /// test vector pin this mapping.
    #[must_use]
    pub fn from_fman_id(fman_id: nostr::PublicKey) -> Self {
        let digest = Sha256::new()
            .chain_update(FMAN_NAME_DOMAIN)
            .chain_update(fman_id.to_bytes())
            .finalize();
        let words = Petnames::medium();
        let adjective = words.adjectives[index(&digest[..8], words.adjectives.len())];
        let noun = words.nouns[index(&digest[8..16], words.nouns.len())];
        Self(format!("{adjective}-{noun}"))
    }

    /// Borrow the stable display form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FmanName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

fn index(bytes: &[u8], word_count: usize) -> usize {
    let bytes: [u8; 8] = bytes.try_into().expect("name digest slice is eight bytes");
    let word_count = u64::try_from(word_count).expect("petname word list length fits u64");
    usize::try_from(u64::from_be_bytes(bytes) % word_count)
        .expect("word-list index originated as usize")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_identity_has_a_golden_name() {
        let id = nostr::PublicKey::parse(
            "9384d5bef4d90f491d316a2b786cd1477ea1dde563ae10d42f8e3e270f249b85",
        )
        .unwrap();
        assert_eq!(FmanName::from_fman_id(id).as_str(), "blissful-chiffchaff");
    }

    #[test]
    fn medium_lists_always_produce_canonical_two_word_names() {
        let words = Petnames::medium();
        assert!(!words.adjectives.is_empty());
        assert!(!words.nouns.is_empty());
        for word in words.adjectives.iter().chain(words.nouns.iter()) {
            assert!(
                !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_lowercase()),
                "petname medium word is not canonical: {word:?}",
            );
        }
    }
}
