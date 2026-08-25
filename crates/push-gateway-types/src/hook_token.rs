use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::{RngCore, rngs::OsRng};
use sha2::{Digest, Sha256};

/// Bearer token returned only when a hook is created.
#[derive(Clone, Eq, PartialEq)]
pub struct HookToken(String);

impl HookToken {
    /// Generates a new unguessable URL-safe hook token.
    #[must_use]
    pub fn generate() -> Self {
        Self(random_url_token(32))
    }

    /// Creates a hook token from a caller-supplied URL path segment.
    #[must_use]
    pub fn from_path_segment(token: String) -> Self {
        Self(token)
    }

    /// Returns the hook token as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the SHA-256 hash stored for this token.
    #[must_use]
    pub fn hash_hex(&self) -> String {
        hex::encode(Sha256::digest(self.0.as_bytes()))
    }
}

impl std::fmt::Debug for HookToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("HookToken(<redacted>)")
    }
}

/// Generates a random URL-safe opaque token/id with the requested byte entropy.
#[must_use]
pub fn random_url_token(bytes: usize) -> String {
    let mut random = vec![0; bytes];
    OsRng.fill_bytes(&mut random);
    URL_SAFE_NO_PAD.encode(random)
}
