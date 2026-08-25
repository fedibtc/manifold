//! AES-256-GCM confinement for guardian telemetry bearer and invite material.

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead as _, KeyInit as _, Payload},
};
use rand::RngCore as _;

/// Encryption/decryption failure intentionally carrying no secret detail.
#[derive(Debug)]
pub(crate) struct TelemetryCryptoError;

/// Cloneable encryption boundary. Key material and ciphertext never implement
/// application-level `Debug` together.
#[derive(Clone)]
pub(crate) struct TelemetrySecretCipher(Aes256Gcm);

impl TelemetrySecretCipher {
    pub(crate) fn new(key: &[u8; 32]) -> Self {
        Self(Aes256Gcm::new_from_slice(key).expect("AES-256 key has fixed length"))
    }

    pub(crate) fn encrypt(
        &self,
        plaintext: &[u8],
        associated_data: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>), TelemetryCryptoError> {
        let mut nonce = [0_u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let ciphertext = self
            .0
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: associated_data,
                },
            )
            .map_err(|_| TelemetryCryptoError)?;
        Ok((nonce.to_vec(), ciphertext))
    }

    pub(crate) fn decrypt(
        &self,
        nonce: &[u8],
        ciphertext: &[u8],
        associated_data: &[u8],
    ) -> Result<Vec<u8>, TelemetryCryptoError> {
        if nonce.len() != 12 {
            return Err(TelemetryCryptoError);
        }
        self.0
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: associated_data,
                },
            )
            .map_err(|_| TelemetryCryptoError)
    }
}

impl std::fmt::Debug for TelemetrySecretCipher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TelemetrySecretCipher([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ciphertext_is_randomized_and_bound_to_target_identity() {
        let cipher = TelemetrySecretCipher::new(&[7; 32]);
        let first = cipher.encrypt(b"secret", b"target:a").unwrap();
        let second = cipher.encrypt(b"secret", b"target:a").unwrap();
        assert_ne!(first, second);
        assert_eq!(
            cipher.decrypt(&first.0, &first.1, b"target:a").unwrap(),
            b"secret"
        );
        assert!(cipher.decrypt(&first.0, &first.1, b"target:b").is_err());
        assert_eq!(format!("{cipher:?}"), "TelemetrySecretCipher([REDACTED])");
    }
}
