use bip39::{Language, Mnemonic};
use hkdf::Hkdf;
use secp256k1::{Keypair, SECP256K1, SecretKey, XOnlyPublicKey};
use sha2::Sha256;

use fedi_decentralized_service_fleet_manager::{SeatId, TelemetryCapability};

const SERVICE_NOSTR_INFO: &[u8] = b"fman/v1/service-nostr";
const SERVICE_SIGN_INFO: &[u8] = b"fman/v1/service-sign";
const IROH_INFO: &[u8] = b"fman/v1/iroh";
const SEAT_INFO_PREFIX: &str = "fman/v1/seat/";
const WALLET_INFO: &[u8] = b"fman/v1/payment-wallet";
const GUARDIAN_FEE_INFO: &[u8] = b"fman/v1/guardian-fee";
const NOSTR_BACKUP_INFO: &[u8] = b"fman/v1/nostr-backup";
const NOSTR_BACKUP_TAG_INFO: &[u8] = b"fman/v1/nostr-backup-tag";
const NOSTR_BACKUP_ENCRYPTION_INFO: &[u8] = b"fman/v1/nostr-backup-encryption";
const TELEMETRY_INFO: &str = "fman/v1/telemetry";

/// BIP-39 root mnemonic for one Fleet Manager install.
pub struct RootMnemonic {
    mnemonic: Mnemonic,
    seed: [u8; 64],
}

impl RootMnemonic {
    /// Generate a fresh 12-word English root mnemonic.
    pub fn generate() -> Result<Self, bip39::Error> {
        Mnemonic::generate_in(Language::English, 12).map(Self::from_mnemonic)
    }

    /// Parse a stored English mnemonic.
    pub fn parse(phrase: &str) -> Result<Self, bip39::Error> {
        Mnemonic::parse_in_normalized(Language::English, phrase).map(Self::from_mnemonic)
    }

    pub fn phrase(&self) -> String {
        self.mnemonic.to_string()
    }

    pub fn derive_service_nostr_secret_key(&self) -> SecretKey {
        self.derive_secp_key(SERVICE_NOSTR_INFO)
    }

    pub fn derive_service_nostr_pubkey(&self) -> XOnlyPublicKey {
        let (pubkey, _) = self
            .derive_service_nostr_secret_key()
            .x_only_public_key(SECP256K1);
        pubkey
    }

    pub fn derive_service_nostr_keys(&self) -> nostr_sdk::Keys {
        nostr_sdk::Keys::new(
            nostr_sdk::SecretKey::from_slice(
                &self.derive_service_nostr_secret_key().secret_bytes(),
            )
            .expect("HKDF-derived nostr key is valid"),
        )
    }

    /// BIP-340 keypair the daemon signs commitment responses with; its
    /// public key is the locator `service_pubkey` FIs verify against.
    pub fn derive_service_signing_key(&self) -> Keypair {
        self.derive_secp_key(SERVICE_SIGN_INFO).keypair(SECP256K1)
    }

    /// X-only public key for [`Self::derive_service_signing_key`].
    pub fn derive_service_pubkey(&self) -> XOnlyPublicKey {
        self.derive_service_signing_key().x_only_public_key().0
    }

    /// Identity that authors and decrypts the encrypted Nostr backup
    /// documents ([SPEC-nostr-backup-restore]). Deliberately not the service
    /// key: discovery and trust surfaces stay unlinked from recovery
    /// material, so an observer cannot resolve an FMan's backup coordinate
    /// from its advertisement.
    ///
    /// [SPEC-nostr-backup-restore]: ../../specs/SPEC-nostr-backup-restore.md
    pub fn derive_nostr_backup_keys(&self) -> nostr_sdk::Keys {
        nostr_sdk::Keys::new(
            nostr_sdk::SecretKey::from_slice(&self.derive_32(NOSTR_BACKUP_INFO))
                .expect("HKDF-derived nostr key is valid"),
        )
    }

    /// Key that blinds a backup event's addressable coordinate.
    ///
    /// Separate from the signing key above because the coordinate is a public
    /// tag computed over a seat id the FI who bought that seat already holds:
    /// an unblinded coordinate would let that FI search the relays for its own
    /// seat and so resolve the backup identity this split exists to hide.
    pub fn derive_nostr_backup_tag_key(&self) -> [u8; 32] {
        self.derive_32(NOSTR_BACKUP_TAG_INFO)
    }

    /// Symmetric key that seals backup event payloads
    /// (XChaCha20-Poly1305). Separate from the signing key: signing proves
    /// authorship to the relay, sealing hides content from it, and neither
    /// job should be able to leak the other's key.
    pub fn derive_nostr_backup_encryption_key(&self) -> [u8; 32] {
        self.derive_32(NOSTR_BACKUP_ENCRYPTION_INFO)
    }

    pub fn derive_iroh_secret_key(&self) -> iroh::SecretKey {
        iroh::SecretKey::from_bytes(&self.derive_32(IROH_INFO))
    }

    /// Root secret for the payment wallet (fman-fedimint); every
    /// per-federation wallet key and locked-quote note key derives from it,
    /// so wallet funds are recoverable from this mnemonic alone.
    pub fn derive_wallet_secret(&self) -> crate::wallet::WalletSecret {
        let hkdf = Hkdf::<Sha256>::new(None, &self.seed);
        let mut out = [0_u8; 64];
        hkdf.expand(WALLET_INFO, &mut out)
            .expect("HKDF-SHA256 supports 64-byte output");
        crate::wallet::WalletSecret(out)
    }

    /// Root secret of the guardian-fee clients. A guarded federation's
    /// guardian client and its payment client live in the same wallet under
    /// the same mnemonic, so they must not share a root: fedimint derives
    /// mint note secrets sequentially from it, and one root across two
    /// databases of one federation collides their issuance
    /// ([ARCH-fleet-manager-identity](../../specs/ARCH-fleet-manager-identity.md)).
    pub fn derive_guardian_fee_secret(&self) -> crate::wallet::WalletSecret {
        let hkdf = Hkdf::<Sha256>::new(None, &self.seed);
        let mut out = [0_u8; 64];
        hkdf.expand(GUARDIAN_FEE_INFO, &mut out)
            .expect("HKDF-SHA256 supports 64-byte output");
        crate::wallet::WalletSecret(out)
    }

    /// The seat's guardian-fee remittance account key.
    ///
    /// Scoped to the seat and nothing else: deliberately not the federation
    /// id, and deliberately not the stability-pool module's own derivation
    /// from the client root, because both only exist once DKG has finished.
    /// The account has to be committed to *before* the ceremony so the
    /// recipient list can be fixed at federation birth
    /// ([SPEC-guardian-fee-policy](../../specs/SPEC-guardian-fee-policy.md)),
    /// which a post-DKG derivation cannot do.
    pub fn derive_guardian_fee_account_key(
        &self,
        seat_id: &SeatId,
    ) -> crate::guardian_fee::GuardianFeeAccountKey {
        let info = format!("{SEAT_INFO_PREFIX}{seat_id}/guardian-fee-account");
        crate::guardian_fee::GuardianFeeAccountKey::from_secret_bytes(
            &self.derive_32(info.as_bytes()),
        )
    }

    /// FMan-wide bearer for seat discovery, metrics, and safe events.
    ///
    /// The durable generation permits explicit global rotation without
    /// storing the bearer itself.
    pub fn derive_telemetry_capability(&self, generation: u64) -> TelemetryCapability {
        let info = format!("{TELEMETRY_INFO}/{generation}");
        TelemetryCapability::from_bytes(self.derive_32(info.as_bytes()))
    }

    /// The seat's two iroh endpoint keys, handed to its fedimintd so seat
    /// NodeIds survive daemon restarts. Scoped so the process-spawning
    /// layers hold material for exactly one seat, nothing else.
    pub fn derive_seat_keys(&self, seat_id: &SeatId) -> SeatKeys {
        // Keep key identity on the content-derived seat id, not the sequential
        // seat_no: key uniqueness must not depend on allocator discipline.
        let key = |purpose: &str| {
            let info = format!("{SEAT_INFO_PREFIX}{seat_id}/{purpose}");
            iroh::SecretKey::from_bytes(&self.derive_32(info.as_bytes()))
        };
        SeatKeys {
            iroh_api: key("iroh-api"),
            iroh_p2p: key("iroh-p2p"),
            api_auth: hex::encode(
                self.derive_32(format!("{SEAT_INFO_PREFIX}{seat_id}/api-auth").as_bytes()),
            ),
        }
    }

    fn from_mnemonic(mnemonic: Mnemonic) -> Self {
        // PBKDF2 over the mnemonic is expensive; compute the seed once.
        let seed = mnemonic.to_seed("");
        Self { mnemonic, seed }
    }

    fn derive_secp_key(&self, info: &[u8]) -> SecretKey {
        // Invalid with probability ~2^-128 (a UUID collision is likelier);
        // treated as unreachable (ARCH-fleet-manager-identity).
        SecretKey::from_byte_array(&self.derive_32(info))
            .expect("HKDF output is a valid secp256k1 scalar")
    }

    fn derive_32(&self, info: &[u8]) -> [u8; 32] {
        let hkdf = Hkdf::<Sha256>::new(None, &self.seed);
        let mut out = [0_u8; 32];
        hkdf.expand(info, &mut out)
            .expect("HKDF-SHA256 supports 32-byte output");
        out
    }
}

/// One seat's derived key material ([`RootMnemonic::derive_seat_keys`]).
#[derive(Clone)]
pub struct SeatKeys {
    pub iroh_api: iroh::SecretKey,
    pub iroh_p2p: iroh::SecretKey,
    pub api_auth: String,
}

impl std::fmt::Debug for SeatKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SeatKeys(redacted)")
    }
}

#[cfg(test)]
#[path = "../tests/identity.rs"]
mod tests;
