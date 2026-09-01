//! Purpose-built encrypted FI recovery document.
//!
//! The plaintext contract is governed by `SPEC-fi-backup-payload`. It is not
//! exposed at the public API boundary: callers can only transport sealed bytes.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use bitcoin_hashes::{Hash as _, sha256};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead as _, AeadCore as _, KeyInit as _, OsRng, Payload},
};
use fedi_decentralized_service_fleet_manager::{InviteCode, Locator, SeatId};
use fedi_decentralized_service_liquidity_manager::RequestLiquidityDetailsCommitmentV1;
use fedimint_derive_secret::{ChildId, DerivableSecret};
use nostr_sdk::{Keys, PublicKey, SecretKey};
use serde::{Deserialize, Serialize};

use crate::{FiError, FiResult};

pub(crate) const FI_BACKUP_EVENT_KIND: u16 = 37706;
pub(crate) const FI_BACKUP_D_TAG: &str = "fedi-fi-backup:v1";
pub(crate) const BACKUP_SCHEMA_VERSION: u32 = 1;
const PADDED_PLAINTEXT_LEN: usize = 32 * 1024;
const LEN_PREFIX: usize = 4;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const DOCUMENT_CAPACITY: usize = PADDED_PLAINTEXT_LEN - LEN_PREFIX;
const FI_BACKUP_NOSTR_CHILD_ID: ChildId = ChildId(1);
const FI_BACKUP_ENCRYPTION_CHILD_ID: ChildId = ChildId(2);
const AAD: &[u8] = b"fedi-fi-backup/document/v1";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FiBackupPayload {
    pub(crate) schema_version: u32,
    pub(crate) snapshot_generation: u64,
    pub(crate) federation_invite: InviteCode,
    pub(crate) seats: Vec<FiBackupSeat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) liquidity: Option<RequestLiquidityDetailsCommitmentV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FiBackupSeat {
    pub(crate) fman_identity: PublicKey,
    pub(crate) seat_id: SeatId,
    pub(crate) locator: Locator,
}

pub(crate) struct PreparedFiBackup {
    pub(crate) payload: FiBackupPayload,
    pub(crate) document_hash: sha256::Hash,
    pub(crate) created_at: u64,
}

/// Opaque, authenticated encrypted FI recovery document.
///
/// Its bytes are standard base64 of `nonce || ciphertext || tag` and always
/// decode to the same fixed length. It deliberately has no `Debug` or
/// serialization implementation.
pub(crate) struct EncryptedFiBackup {
    content: String,
}

impl EncryptedFiBackup {
    pub(crate) fn from_bytes(bytes: Vec<u8>) -> FiResult<Self> {
        let content =
            String::from_utf8(bytes).map_err(|_| invalid("sealed document is not UTF-8 base64"))?;
        validate_blob(&content)?;
        Ok(Self { content })
    }

    pub(crate) fn content(&self) -> &str {
        &self.content
    }
}

pub(crate) struct FiBackupKeys {
    author: Keys,
    cipher: XChaCha20Poly1305,
}

impl FiBackupKeys {
    pub(crate) fn derive(root: &DerivableSecret) -> Self {
        let author_keypair = root
            .child_key(FI_BACKUP_NOSTR_CHILD_ID)
            .to_secp_key(&fedimint_core::secp256k1::Secp256k1::new());
        let author = Keys::new(
            SecretKey::from_slice(&author_keypair.secret_key().secret_bytes())
                .expect("Fedimint derived a valid secp256k1 secret"),
        );
        let content = root
            .child_key(FI_BACKUP_ENCRYPTION_CHILD_ID)
            .to_chacha20_poly1305_key_raw();
        Self {
            author,
            cipher: XChaCha20Poly1305::new_from_slice(&content).expect("32-byte key"),
        }
    }

    pub(crate) fn author(&self) -> &Keys {
        &self.author
    }
    pub(crate) fn public_key(&self) -> PublicKey {
        self.author.public_key()
    }

    pub(crate) fn seal(&self, payload: &FiBackupPayload) -> FiResult<EncryptedFiBackup> {
        let mut encoded = Vec::new();
        ciborium::into_writer(payload, &mut encoded)
            .map_err(|_| invalid("could not encode recovery payload"))?;
        if encoded.len() > DOCUMENT_CAPACITY {
            return Err(invalid("recovery payload exceeds the 32 KiB document"));
        }
        let mut plaintext = vec![0u8; PADDED_PLAINTEXT_LEN];
        plaintext[..LEN_PREFIX].copy_from_slice(&(encoded.len() as u32).to_le_bytes());
        plaintext[LEN_PREFIX..LEN_PREFIX + encoded.len()].copy_from_slice(&encoded);
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &plaintext,
                    aad: AAD,
                },
            )
            .map_err(|_| invalid("could not seal recovery payload"))?;
        let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        blob.extend_from_slice(&nonce);
        blob.extend_from_slice(&ciphertext);
        Ok(EncryptedFiBackup {
            content: BASE64.encode(blob),
        })
    }

    pub(crate) fn open(&self, backup: &EncryptedFiBackup) -> FiResult<FiBackupPayload> {
        let blob = validate_blob(backup.content())?;
        let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
        let plaintext = self
            .cipher
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: AAD,
                },
            )
            .map_err(|_| invalid("document authentication failed"))?;
        if plaintext.len() != PADDED_PLAINTEXT_LEN {
            return Err(invalid("wrong plaintext length"));
        }
        let len =
            u32::from_le_bytes(plaintext[..LEN_PREFIX].try_into().expect("four bytes")) as usize;
        if len > DOCUMENT_CAPACITY {
            return Err(invalid("malformed length frame"));
        }
        let end = LEN_PREFIX + len;
        if plaintext[end..].iter().any(|byte| *byte != 0) {
            return Err(invalid("non-zero frame padding"));
        }
        let mut cursor = std::io::Cursor::new(&plaintext[LEN_PREFIX..end]);
        let payload: FiBackupPayload =
            ciborium::from_reader(&mut cursor).map_err(|_| invalid("payload is not valid CBOR"))?;
        if cursor.position() as usize != len {
            return Err(invalid("frame contains more than one CBOR item"));
        }
        validate_payload(&payload)?;
        Ok(payload)
    }
}

pub(crate) fn semantic_hash(payload: &FiBackupPayload) -> FiResult<sha256::Hash> {
    let mut semantic = payload.clone();
    semantic.snapshot_generation = 0;
    encoded_hash(&semantic)
}

pub(crate) fn document_hash(payload: &FiBackupPayload) -> FiResult<sha256::Hash> {
    encoded_hash(payload)
}

fn encoded_hash(payload: &FiBackupPayload) -> FiResult<sha256::Hash> {
    let mut bytes = Vec::new();
    ciborium::into_writer(payload, &mut bytes)
        .map_err(|_| invalid("could not encode recovery payload"))?;
    Ok(sha256::Hash::hash(&bytes))
}

fn validate_payload(payload: &FiBackupPayload) -> FiResult<()> {
    if payload.schema_version != BACKUP_SCHEMA_VERSION {
        return Err(invalid("unsupported schema version"));
    }
    if payload.snapshot_generation == 0 {
        return Err(invalid("snapshot generation is zero"));
    }
    if payload.seats.is_empty() {
        return Err(invalid("formed federation has no seats"));
    }
    let mut identities = std::collections::BTreeSet::new();
    let mut seat_ids = std::collections::BTreeSet::new();
    let mut service_keys = std::collections::BTreeSet::new();
    for seat in &payload.seats {
        if !identities.insert(seat.fman_identity)
            || !seat_ids.insert(seat.seat_id.clone())
            || !service_keys.insert(seat.locator.service_pubkey)
        {
            return Err(invalid("backup seats are not unique"));
        }
    }
    Ok(())
}

fn validate_blob(content: &str) -> FiResult<Vec<u8>> {
    let blob = BASE64
        .decode(content)
        .map_err(|_| invalid("document is not standard base64"))?;
    if blob.len() != NONCE_LEN + PADDED_PLAINTEXT_LEN + TAG_LEN {
        return Err(invalid("sealed document has the wrong fixed length"));
    }
    Ok(blob)
}

fn invalid(reason: &str) -> FiError {
    FiError::Storage(format!("invalid encrypted FI backup: {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fedi_decentralized_service_fleet_manager::QuoteId;
    use fedi_iroh_rpc::iroh::{EndpointAddr, SecretKey as IrohSecretKey};
    use fedimint_derive_secret::DerivableSecret;

    fn keys(marker: u8) -> FiBackupKeys {
        FiBackupKeys::derive(&DerivableSecret::new_root(&[marker; 64], b"fi-backup-test"))
    }

    #[test]
    fn fixed_frame_round_trip_is_randomized_and_authenticated() {
        let payload = FiBackupPayload {
            schema_version: 1,
            snapshot_generation: 4,
            federation_invite: InviteCode("invite".into()),
            seats: vec![],
            liquidity: None,
        };
        let mut payload = payload;
        // Payload validation happens on open; use a fixture seat in DB-level tests.
        payload.seats.push(FiBackupSeat {
            fman_identity: keys(3).public_key(),
            seat_id: SeatId::from(QuoteId([1; 32])),
            locator: Locator::new(
                EndpointAddr::new(IrohSecretKey::from_bytes(&[2; 32]).public()),
                secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &[3; 32])
                    .unwrap()
                    .x_only_public_key()
                    .0,
            ),
        });
        let backup_keys = keys(7);
        let first = backup_keys.seal(&payload).unwrap();
        let second = backup_keys.seal(&payload).unwrap();
        assert_ne!(first.content(), second.content());
        assert_eq!(
            BASE64.decode(first.content()).unwrap().len(),
            NONCE_LEN + PADDED_PLAINTEXT_LEN + TAG_LEN
        );
        assert_eq!(backup_keys.open(&first).unwrap(), payload);
        assert!(keys(8).open(&first).is_err());
        let mut blob = BASE64.decode(first.content()).unwrap();
        blob[NONCE_LEN + 7] ^= 1;
        assert!(
            backup_keys
                .open(&EncryptedFiBackup {
                    content: BASE64.encode(blob)
                })
                .is_err()
        );
    }

    #[test]
    fn oversize_is_rejected_without_slicing() {
        let payload = FiBackupPayload {
            schema_version: 1,
            snapshot_generation: 1,
            federation_invite: InviteCode("x".repeat(PADDED_PLAINTEXT_LEN)),
            seats: vec![],
            liquidity: None,
        };
        assert!(keys(1).seal(&payload).is_err());
    }
}
