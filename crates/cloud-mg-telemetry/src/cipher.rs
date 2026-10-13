use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead as _, KeyInit as _, Payload},
};
use rand::RngCore as _;

/// Encryption boundary whose diagnostics never include secret material.
#[derive(Clone)]
pub(crate) struct SecretCipher(Aes256Gcm);

impl SecretCipher {
    pub(crate) fn new(key: &[u8; 32]) -> Self {
        Self(Aes256Gcm::new_from_slice(key).expect("fixed key length"))
    }
    pub(crate) fn encrypt(
        &self,
        plaintext: &[u8],
        aad: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>), CipherError> {
        let mut nonce = [0; 12];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let ciphertext = self
            .0
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| CipherError)?;
        Ok((nonce.to_vec(), ciphertext))
    }

    pub(crate) fn decrypt(
        &self,
        nonce: &[u8],
        ciphertext: &[u8],
        aad: &[u8],
    ) -> Result<Vec<u8>, CipherError> {
        if nonce.len() != 12 {
            return Err(CipherError);
        }
        self.0
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| CipherError)
    }
}

impl std::fmt::Debug for SecretCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretCipher([REDACTED])")
    }
}

/// Sanitized encryption failure.
#[derive(Debug, thiserror::Error)]
#[error("secret encryption failed")]
pub(crate) struct CipherError;
