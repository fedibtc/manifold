use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedimint_derive_secret::DerivableSecret;
use secp256k1::{Keypair, Secp256k1, SecretKey};
use zeroize::Zeroizing;

use crate::backup::FiBackupKeys;
use crate::{FiId, FiSignature};

/// FI keys derived from one consumer-scoped root.
pub(crate) struct FiKeys {
    root: DerivableSecret,
    protocol: Keypair,
    environment: ManifoldEnvironment,
}

impl FiKeys {
    pub(crate) fn new(root: DerivableSecret, environment: ManifoldEnvironment) -> Self {
        let protocol = Keypair::from_secret_key(&Secp256k1::new(), &secp_secret(&root));
        Self {
            root,
            protocol,
            environment,
        }
    }

    pub(crate) fn fi_id(&self) -> FiId {
        FiId(self.protocol.x_only_public_key().0)
    }

    pub(crate) fn sign_digest(&self, digest: [u8; 32]) -> FiSignature {
        FiSignature(Secp256k1::new().sign_schnorr_no_aux_rand(&digest, &self.protocol))
    }

    pub(crate) fn backup_keys(&self) -> FiBackupKeys {
        let backup_root = Zeroizing::new(self.root.to_random_bytes::<32>());
        FiBackupKeys::derive(backup_root.as_ref(), self.environment)
            .expect("a scoped FI root derives valid backup keys")
    }
}

fn secp_secret(root: &DerivableSecret) -> SecretKey {
    let keypair = root
        .clone()
        .to_secp_key(&fedimint_core::secp256k1::Secp256k1::new());
    SecretKey::from_byte_array(&keypair.secret_key().secret_bytes())
        .expect("Fedimint derived a valid secp256k1 secret")
}

#[cfg(test)]
mod tests {
    use fedimint_derive_secret::ChildId;

    use super::*;

    #[test]
    fn scoped_root_keeps_fedi_child_17_protocol_identity() {
        let fi_root = DerivableSecret::new_root(&[42; 64], b"fedi-fi-identity-compatibility")
            .child_key(ChildId(17));
        let legacy = fi_root
            .clone()
            .to_secp_key(&fedimint_core::secp256k1::Secp256k1::new())
            .x_only_public_key()
            .0
            .serialize();
        let current = FiKeys::new(fi_root, ManifoldEnvironment::Staging)
            .fi_id()
            .0
            .serialize();

        assert_eq!(current, legacy);
    }
}
