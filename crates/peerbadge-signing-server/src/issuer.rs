use std::path::Path;

use anyhow::{Context as _, ensure};
use fedi_decentralized_manifold_environment::ManifoldEnvironmentProfile;
use nostr_sdk::Keys;
use peerbadge_protocol::{IssuerAuthority, IssuerContext, IssuerSecretKeys, RevocationLocation};

/// Issuance material and its public, identity-signed authority.
pub struct IssuerMaterial {
    pub context: IssuerContext,
    pub authority: IssuerAuthority,
    pub authority_json: String,
    pub keys: Keys,
}

impl IssuerMaterial {
    /// Load explicit keys, or the canonical public test fixture. Production has
    /// no fallback. Pinned fixture documents are returned byte-for-byte.
    pub fn load(
        profile: &ManifoldEnvironmentProfile,
        key_file: Option<&Path>,
    ) -> anyhow::Result<Self> {
        let json = match key_file {
            Some(path) => std::fs::read_to_string(path).context("read issuer key file")?,
            None => profile
                .test_issuer_secret_keys()
                .context("--key-file is required for production")?
                .to_owned(),
        };
        // Do not attach SDK/serde errors: key material can appear in diagnostics.
        let secret: IssuerSecretKeys =
            serde_json::from_str(&json).map_err(|_| anyhow::anyhow!("invalid issuer key file"))?;
        let context = IssuerContext::import_secret_key(&secret)
            .map_err(|_| anyhow::anyhow!("invalid issuer secret keys"))?;
        let keys = Keys::parse(&secret.issuer_id_secret_key)
            .map_err(|_| anyhow::anyhow!("invalid issuer identity key"))?;
        let minted = context
            .issuer_authority(
                profile
                    .nostr_relays()
                    .as_urls()
                    .iter()
                    .map(|url| RevocationLocation {
                        protocol: "nostr".to_owned(),
                        location: url.to_string(),
                    })
                    .collect(),
            )
            .map_err(|_| anyhow::anyhow!("cannot create issuer authority"))?;
        let pinned = profile
            .pinned_issuer_authorities()
            .iter()
            .find_map(|document| {
                let authority: IssuerAuthority = serde_json::from_str(document).ok()?;
                (authority.issuer.issuer_id_pubkey == minted.issuer.issuer_id_pubkey
                    && authority.issuer.issuance_key == minted.issuer.issuance_key)
                    .then_some((authority, (*document).to_owned()))
            });
        let (authority, authority_json) = match pinned {
            Some(pinned) => pinned,
            None => {
                ensure!(
                    key_file.is_some(),
                    "canonical test issuer authority is missing"
                );
                // The SDK uses randomized authority signatures. Canonicalize
                // only unpinned proofs with BIP-340's deterministic signing so
                // separate publication and serve processes return identical JSON.
                let mut minted = minted;
                let digest = minted
                    .digest()
                    .map_err(|_| anyhow::anyhow!("cannot digest issuer authority"))?;
                minted.proof.signature = nostr_sdk::SECP256K1.sign_schnorr_no_aux_rand(
                    &nostr_sdk::secp256k1::Message::from_digest(digest.into()),
                    keys.key_pair(nostr_sdk::SECP256K1),
                );
                let document = serde_json::to_string(&minted)?;
                (minted, document)
            }
        };
        authority
            .verify()
            .map_err(|_| anyhow::anyhow!("invalid issuer authority"))?;
        Ok(Self {
            context,
            authority,
            authority_json,
            keys,
        })
    }
}
