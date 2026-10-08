use std::{fs::File, io::Read as _, path::Path};

use anyhow::{Context as _, ensure};
use fedi_decentralized_manifold_environment::{ManifoldEnvironment, ManifoldEnvironmentProfile};
use fedi_decentralized_nostr::attester::NOSTR_REVOCATION_LOCATION_PROTOCOL;
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
            Some(path) => {
                let mut file = File::open(path).context("open issuer key file")?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    ensure!(
                        file.metadata()
                            .context("inspect issuer key file")?
                            .permissions()
                            .mode()
                            & 0o077
                            == 0,
                        "issuer key file must not be accessible by group or others"
                    );
                }
                let mut json = String::new();
                file.read_to_string(&mut json)
                    .context("read issuer key file")?;
                json
            }
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
        if profile.environment() == ManifoldEnvironment::Production {
            for environment in [
                ManifoldEnvironment::Development,
                ManifoldEnvironment::Staging,
            ] {
                ensure!(
                    !environment
                        .profile()?
                        .peer_badge_issuer_identities()
                        .contains(&keys.public_key()),
                    "production issuer must not use a public Development or Staging fixture identity"
                );
            }
        }
        let minted = context
            .issuer_authority(
                profile
                    .nostr_relays()
                    .as_urls()
                    .iter()
                    .map(|url| RevocationLocation {
                        protocol: NOSTR_REVOCATION_LOCATION_PROTOCOL.to_owned(),
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

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn private_key_file(json: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            file.as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        file.write_all(json.as_bytes()).unwrap();
        file
    }

    #[cfg(unix)]
    #[test]
    fn issuer_key_file_accepts_private_modes_and_rejects_each_group_other_bit() {
        use std::os::unix::fs::PermissionsExt as _;

        let profile = ManifoldEnvironment::Development.profile().unwrap();
        let file = private_key_file(profile.test_issuer_secret_keys().unwrap());
        for mode in [0o400, 0o600] {
            file.as_file()
                .set_permissions(std::fs::Permissions::from_mode(mode))
                .unwrap();
            let material = IssuerMaterial::load(&profile, Some(file.path())).unwrap();
            assert_eq!(
                material.authority_json,
                profile.pinned_issuer_authorities()[0]
            );
        }
        for bit in [0o040, 0o020, 0o010, 0o004, 0o002, 0o001] {
            file.as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600 | bit))
                .unwrap();
            let error = IssuerMaterial::load(&profile, Some(file.path()))
                .err()
                .unwrap();
            assert_eq!(
                error.to_string(),
                "issuer key file must not be accessible by group or others",
                "accepted group/other permission bit {bit:o}"
            );
        }
    }

    #[test]
    fn production_rejects_both_private_fixture_files() {
        let production = ManifoldEnvironment::Production.profile().unwrap();
        for environment in [
            ManifoldEnvironment::Development,
            ManifoldEnvironment::Staging,
        ] {
            let profile = environment.profile().unwrap();
            let file = private_key_file(profile.test_issuer_secret_keys().unwrap());
            let error = IssuerMaterial::load(&production, Some(file.path()))
                .err()
                .unwrap();
            assert_eq!(
                error.to_string(),
                "production issuer must not use a public Development or Staging fixture identity"
            );
        }
    }

    #[test]
    fn production_accepts_generated_keys_but_rejects_fixture_identities_with_new_issuance_keys() {
        let production = ManifoldEnvironment::Production.profile().unwrap();
        let context = IssuerContext::generate().unwrap();
        let mut secret = context.export_secret_key().unwrap();
        let file = private_key_file(&serde_json::to_string(&secret).unwrap());
        let material = IssuerMaterial::load(&production, Some(file.path())).unwrap();
        assert_eq!(
            material.keys.public_key(),
            Keys::parse(&secret.issuer_id_secret_key)
                .unwrap()
                .public_key()
        );
        material.authority.verify().unwrap();
        assert!(
            material
                .authority
                .issuer
                .revocation
                .iter()
                .all(|location| location.protocol == NOSTR_REVOCATION_LOCATION_PROTOCOL)
        );

        for environment in [
            ManifoldEnvironment::Development,
            ManifoldEnvironment::Staging,
        ] {
            let profile = environment.profile().unwrap();
            let fixture: IssuerSecretKeys =
                serde_json::from_str(profile.test_issuer_secret_keys().unwrap()).unwrap();
            secret.issuer_id_secret_key = fixture.issuer_id_secret_key;
            let file = private_key_file(&serde_json::to_string(&secret).unwrap());
            let error = IssuerMaterial::load(&production, Some(file.path()))
                .err()
                .unwrap();
            assert_eq!(
                error.to_string(),
                "production issuer must not use a public Development or Staging fixture identity"
            );
        }
    }
}
