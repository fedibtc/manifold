//! Trust-material fabrication for the live single-flow harness.
//!
//! The live tests run the daemon's production verification pipeline for real;
//! only the two `--trust-fixtures` inputs (federation preview, FMan trust
//! material) are fabricated here. Every fabricated artifact is genuinely
//! signed: FMan peer attestations and trust-material envelopes with fresh
//! FMan Schnorr keys, credentials and holder authorizations with the shared
//! test issuer whose authority points revocation lookups at the live local
//! Nostr relay.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use fedi_credential_sdk_protocol::{HolderContext, IssuerAuthority, IssuerContext};
use fedi_decentralized_liquidity_manager_daemon::{FederationPreview, PreviewPeer, trust_fixtures};
use fedi_decentralized_service_liquidity_manager::{
    AttestationInstallRequest, BitcoinNetwork, FederationId, FmanEndorsement, FmanPeerAttestation,
    FmanPeerAttestationStatement, FmanSeatBindings, FmanTrustMaterial,
    GetFmanTrustMaterialResponse, GuardianIdentity, HashBytes, HolderAuthorizationEnvelope, PeerId,
    ProtocolV1, Pubkey, SchnorrSignatureProof, Timestamp, Url,
};
use nostr_sdk::Keys;
use nostr_sdk::secp256k1::Message;
use reqwest::Client;

use crate::common::credentials;

use super::daemon::{DaemonLaunch, admin_post_when_available};

/// Trust-material validity window written into fixtures; must stay under the
/// daemon's 3600s `FMAN_TRUST_MATERIAL_MAX_VALIDITY_SECS` bound while leaving
/// the harness enough runway between fixture writing and the request.
const TRUST_MATERIAL_VALIDITY_SECS: u64 = 3_500;

/// Shared trust identities for one live daemon instance.
pub struct LiveTrust {
    pub issuer: IssuerContext,
    pub authority: IssuerAuthority,
    /// Issuer identity accepted by the provider's attester policy.
    pub attester_pubkey_hex: String,
    /// Provider service identity derived from the launch signing key.
    pub provider_pubkey: Pubkey,
}

/// Build the trust identities for a daemon launch; the issuer authority's
/// revocation location points at the live local relay so request-time
/// revocation lookups run for real.
pub fn live_trust(launch: &DaemonLaunch, revocation_relay_url: &str) -> anyhow::Result<LiveTrust> {
    let issuer = credentials::test_issuer_context();
    let authority = credentials::test_issuer_authority(&issuer, revocation_relay_url)?;
    let attester_pubkey_hex = authority.issuer.issuer_id_pubkey.0.to_string();
    let provider_keys = launch.provider_keys()?;
    Ok(LiveTrust {
        issuer,
        authority,
        attester_pubkey_hex,
        provider_pubkey: Pubkey(provider_keys.public_key().to_hex()),
    })
}

/// Install the trusted issuer authority through the Admin API.
///
/// This is operator-configured trust policy and still arrives by upload. The
/// provider's own holder authorization does not: it is enrolled from the relay
/// by [`enrol_provider_authorization`], which must run after setup config has
/// given the daemon a relay to reconcile against.
pub async fn install_issuer_authority(
    http: &Client,
    admin_url: &str,
    trust: &LiveTrust,
) -> anyhow::Result<()> {
    admin_post_when_available(
        http,
        admin_url,
        "attestation_install",
        &serde_json::to_value(AttestationInstallRequest {
            payload: credentials::attestation_payload(&trust.authority)?,
        })?,
    )
    .await?;
    Ok(())
}

/// Publish a Holder authorization for this provider to the live relay and have
/// the daemon enrol it, so the advertisement trust-envelope readiness gate
/// passes.
///
/// Asserting the reconciliation here keeps an enrollment failure legible: the
/// alternative is that every live test fails much later with "advertisement did
/// not publish", which says nothing about why.
pub async fn enrol_provider_authorization(
    http: &Client,
    admin_url: &str,
    trust: &LiveTrust,
    relay_url: &str,
) -> anyhow::Result<()> {
    // Enrollment is durable, so a restart within one test must not enrol a
    // second Holder: that would change the advertisement payload and break the
    // byte-identical republish the restart tests assert on.
    let state = admin_post_when_available(
        http,
        admin_url,
        "get_holder_authorization_state",
        &serde_json::json!({}),
    )
    .await?;
    if state["status"]["state"] == "authorization_observed" {
        return Ok(());
    }

    let holder = HolderContext::generate();
    let credential =
        credentials::issue_credential_for_holder(&trust.issuer, &trust.authority, &holder)?;
    let authorization = credentials::holder_authorization_for_provider(
        &holder,
        &credential,
        &trust.provider_pubkey,
    )?;
    publish_holder_authorization(
        relay_url,
        &holder,
        &authorization,
        &credential,
        &trust.provider_pubkey,
    )
    .await?;

    let outcome = admin_post_when_available(
        http,
        admin_url,
        "refresh_holder_authorizations",
        &serde_json::json!({}),
    )
    .await?;
    // At least one, not exactly one: a test that restarts its daemon calls this
    // again, and the relay still serves the earlier holder's authorization
    // alongside the new one. Both enrol, under separate credential digests.
    anyhow::ensure!(
        outcome["candidates_verified"].as_u64().unwrap_or_default() >= 1,
        "live provider authorization did not enrol: {outcome}"
    );
    Ok(())
}

/// Publish the kind-37705 event the Holder app publishes, with the FLIP tags.
async fn publish_holder_authorization(
    relay_url: &str,
    holder: &HolderContext,
    authorization: &fedi_credential_sdk_protocol::HolderAuthorization,
    credential: &fedi_credential_sdk_protocol::SignedCredential,
    provider_pubkey: &Pubkey,
) -> anyhow::Result<()> {
    use fedi_decentralized_nostr::flip::{
        FLIP_AUTHORIZATION_HASHTAG, HOLDER_AUTHORIZATION_EVENT_KIND, flip_authorization_d_tag,
    };

    let credential_digest = serde_json::to_value(&authorization.authorization.credential_digest)?
        .as_str()
        .context("credential digest serializes as a string")?
        .to_owned();
    let content = serde_json_canonicalizer::to_string(&serde_json::json!({
        "version": 1,
        "holder_id_pubkey": holder.public_key().to_string(),
        "holder_authorization": authorization,
        "signed_credential": credential,
    }))?;

    let client = nostr_sdk::Client::new(Keys::parse(&holder.export_secret_key())?);
    client.add_relay(relay_url).await?;
    client.connect().await;
    client
        .send_event_builder(
            nostr_sdk::EventBuilder::new(
                nostr_sdk::Kind::Custom(HOLDER_AUTHORIZATION_EVENT_KIND),
                content,
            )
            .tags([
                nostr_sdk::Tag::parse([
                    "d",
                    flip_authorization_d_tag(&provider_pubkey.0, &credential_digest).as_str(),
                ])?,
                nostr_sdk::Tag::parse(["t", FLIP_AUTHORIZATION_HASHTAG])?,
                nostr_sdk::Tag::parse(["p", provider_pubkey.0.as_str()])?,
            ]),
        )
        .await?;
    // The relay indexes asynchronously and the daemon fetch is a separate
    // connection, so it must not race the write.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    client.disconnect().await;
    Ok(())
}

/// Signed FMan artifacts baked into the fixture files, kept so tests can
/// exercise revocation against them later.
pub struct TrustFixtureArtifacts {
    /// Backing credential per fixture FMan, in fixture order.
    pub fman_credentials: Vec<fedi_credential_sdk_protocol::SignedCredential>,

    /// A valid admission endorsement from the first fixture FMan, for the
    /// same federation the invite code names.
    pub endorsement: FmanEndorsement,

    /// Per-FMan signed trust material, in fixture order, as each FMan's own
    /// `get_fman_trust_material` would serve it. Requests carry these;
    /// withholding one is how a test makes an identity unanswered.
    pub trust_material: Vec<GetFmanTrustMaterialResponse>,

    /// The coherent preview written to the fixture directory.
    pub preview: FederationPreview,
}

/// Write the preview fixture for the real target federation invite code.
///
/// The preview is the only fixture-substituted trust input; the seat-binding
/// directory it carries and the per-FMan trust material that backs it are real
/// signed material, and the material must be attached to the request before it
/// can be accepted.
/// `module_kinds` is what the previewed config claims the target federation
/// carries. It is not cosmetic: acceptance refuses a stability-pool request
/// whose target has no stability-pool module, so a stability test must name it
/// and a gateway-only test must not.
pub fn write_trust_fixtures(
    trust: &LiveTrust,
    fixtures_dir: &Path,
    invite_code: &str,
    federation_config_hash: &HashBytes,
    module_kinds: &[&str],
) -> anyhow::Result<TrustFixtureArtifacts> {
    let parsed_invite = fedimint_core::invite_code::InviteCode::from_str(invite_code)
        .context("parse target invite code for fixtures")?;
    let federation_id = FederationId(parsed_invite.federation_id().to_string());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before UNIX epoch")?
        .as_secs();

    // Two distinct FMan identities, one previewed peer each.
    let fmans: Vec<(Keys, &str, &str)> = vec![
        (Keys::generate(), "0", "guardian-0"),
        (Keys::generate(), "1", "guardian-1"),
    ];

    let mut seat_attestations = Vec::new();
    let mut fman_credentials = Vec::new();
    let mut material_inputs = Vec::new();
    let mut endorsement = None;
    for (keys, peer_id, guardian) in &fmans {
        let (attestation, holder_authorization, signed_credential) = seat_material(
            trust,
            keys,
            peer_id,
            guardian,
            &federation_id,
            federation_config_hash,
            now,
        )?;
        // The first FMan's consensus-bound attestation and matching holder
        // envelope form the admission endorsement without a second identity.
        if endorsement.is_none() {
            endorsement = Some(FmanEndorsement {
                attestation: attestation.clone(),
                trust: HolderAuthorizationEnvelope {
                    holder_authorization: holder_authorization.clone(),
                    signed_credential: signed_credential.clone(),
                },
            });
        }
        material_inputs.push((
            keys.clone(),
            HolderAuthorizationEnvelope {
                holder_authorization,
                signed_credential: signed_credential.clone(),
            },
        ));
        seat_attestations.push(attestation);
        fman_credentials.push(signed_credential);
    }
    let endorsement = endorsement.context("fixture FMan list is empty")?;

    let seat_bindings = FmanSeatBindings::new(seat_attestations)
        .map_err(|error| anyhow::anyhow!("assemble fixture seat bindings: {error}"))?
        .canonical_string()
        .map_err(|error| anyhow::anyhow!("canonicalize fixture seat bindings: {error}"))?;
    let preview = FederationPreview {
        federation_id,
        federation_config_hash: federation_config_hash.clone(),
        network: BitcoinNetwork::Regtest,
        peers: fmans
            .iter()
            .map(|(_, peer_id, guardian)| PreviewPeer {
                peer_id: PeerId((*peer_id).to_owned()),
                guardian_identity: GuardianIdentity((*guardian).to_owned()),
            })
            .collect(),
        consensus_threshold: 2,
        fman_seat_bindings_metadata: Some(seat_bindings),
        module_kinds: module_kinds.iter().copied().map(str::to_owned).collect(),
    };
    rewrite_preview_fixture(fixtures_dir, invite_code, &preview)?;

    let trust_material = material_inputs
        .into_iter()
        .map(|(keys, envelope)| sign_trust_material(&keys, vec![envelope], now))
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(TrustFixtureArtifacts {
        fman_credentials,
        endorsement,
        trust_material,
        preview,
    })
}

/// Sign one FMan's trust material exactly as its daemon would.
///
/// Built here rather than fetched over RPC: the live harness stands up a real
/// federation but not a real FMan fleet, so the fixture plays the FMan's
/// signing role while every byte the daemon verifies stays real.
pub fn sign_trust_material(
    fman_keys: &Keys,
    holder_authorizations: Vec<HolderAuthorizationEnvelope>,
    now: u64,
) -> anyhow::Result<GetFmanTrustMaterialResponse> {
    let material = FmanTrustMaterial {
        fman_pubkey: Pubkey(fman_keys.public_key().to_hex()),
        issued_at: Timestamp(now),
        expires_at: Timestamp(now + 600),
        public_api_urls: vec![Url(format!("iroh://{}", fman_keys.public_key().to_hex()))],
        holder_authorizations,
    };
    let message = Message::from_digest(material.digest()?);
    Ok(GetFmanTrustMaterialResponse {
        version: ProtocolV1,
        material,
        proof: SchnorrSignatureProof {
            signature: fman_keys.sign_schnorr(&message),
        },
    })
}

/// Rewrite the preview fixture from an already-written artifact set.
///
/// Deliberately does not call [`write_trust_fixtures`] again: that mints fresh
/// FMan identities, and any advertisement already published to the relay would
/// stop matching, turning a seat-binding test into a missing-advertisement one.
pub fn rewrite_preview_fixture(
    fixtures_dir: &Path,
    invite_code: &str,
    preview: &FederationPreview,
) -> anyhow::Result<()> {
    // Read, replace this invite code's entry, write back. The fixture is a map
    // because one daemon can be asked about more than one target federation,
    // and writing a fresh single-entry map would erase a sibling federation's
    // preview. That failure does not look like a missing fixture at request
    // time: it looks like a federation whose invite code cannot be previewed,
    // which is a rejection rather than an error.
    let path = fixtures_dir.join(trust_fixtures::PREVIEWS_FIXTURE_FILENAME);
    let mut previews: HashMap<String, FederationPreview> = match fs::read_to_string(&path) {
        Ok(existing) => serde_json::from_str(&existing).context("parse previews fixture")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
        Err(error) => return Err(error).context("read previews fixture"),
    };
    previews.insert(invite_code.to_owned(), preview.clone());
    fs::write(&path, serde_json::to_string_pretty(&previews)?)
        .context("rewrite previews fixture")?;

    Ok(())
}

/// Publish the attester-authored kind-37704 revocation for one fixture FMan
/// credential to the live local relay, so the daemon's fresh request-time
/// revocation lookup discovers it.
pub async fn publish_revocation(
    relay_ws_url: &str,
    trust: &LiveTrust,
    credential: &fedi_credential_sdk_protocol::SignedCredential,
) -> anyhow::Result<()> {
    use fedi_credential_sdk_protocol::CredentialDigest;
    use fedi_decentralized_liquidity_manager_daemon::revocation::credential_digest_wire_string;
    use fedi_decentralized_nostr::attester::{
        CREDENTIAL_REVOCATION_EVENT_KIND, CREDENTIAL_REVOCATION_HASHTAG,
        credential_revocation_d_tag,
    };

    let revocation = trust.issuer.revoke_credential(credential)?;
    let digest = credential_digest_wire_string(&CredentialDigest(credential.credential.digest()?));
    let issuer_keys = Keys::parse(&trust.issuer.export_secret_key()?.issuer_id_secret_key)?;
    let client = nostr_sdk::Client::new(issuer_keys);
    client.add_relay(relay_ws_url).await?;
    client.connect().await;
    let builder = nostr_sdk::EventBuilder::new(
        nostr_sdk::Kind::Custom(CREDENTIAL_REVOCATION_EVENT_KIND),
        serde_json::to_string(&revocation)?,
    )
    .tags([
        nostr_sdk::Tag::identifier(credential_revocation_d_tag(&digest)),
        nostr_sdk::Tag::hashtag(CREDENTIAL_REVOCATION_HASHTAG),
    ]);
    let output = client.send_event_builder(builder).await?;
    anyhow::ensure!(
        !output.success.is_empty(),
        "relay did not accept the revocation event"
    );
    client.disconnect().await;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
/// Sign one seat attestation and issue the badge its FMan advertises.
fn seat_material(
    trust: &LiveTrust,
    fman_keys: &Keys,
    peer_id: &str,
    guardian: &str,
    federation_id: &FederationId,
    federation_config_hash: &HashBytes,
    now: u64,
) -> anyhow::Result<(
    FmanPeerAttestation,
    fedi_credential_sdk_protocol::HolderAuthorization,
    fedi_credential_sdk_protocol::SignedCredential,
)> {
    let fman_pubkey = Pubkey(fman_keys.public_key().to_hex());
    let account_seed = peer_id.parse::<u8>().unwrap_or(0).saturating_add(1);
    let account_key = bitcoin::secp256k1::PublicKey::from_secret_key(
        bitcoin::secp256k1::SECP256K1,
        &bitcoin::secp256k1::SecretKey::from_slice(&[account_seed; 32])
            .expect("fixed test scalar is valid"),
    );

    let statement = FmanPeerAttestationStatement {
        fman_pubkey: fman_pubkey.clone(),
        federation_id: federation_id.clone(),
        federation_config_hash: federation_config_hash.clone(),
        peer_id: PeerId(peer_id.to_owned()),
        guardian_identity: GuardianIdentity(guardian.to_owned()),
        guardian_fee_account: serde_json::from_value(serde_json::json!({
            "acc_type": "BtcDepositor",
            "pub_keys": [account_key.to_string()],
            "threshold": 1,
        }))
        .expect("test guardian-fee account decodes"),
        issued_at: Timestamp(now),
    };
    let message = Message::from_digest(statement.digest()?);
    let attestation = FmanPeerAttestation {
        version: ProtocolV1,
        attestation: statement,
        proof: SchnorrSignatureProof {
            signature: fman_keys.sign_schnorr(&message),
        },
    };

    let holder = HolderContext::generate();
    let credential =
        credentials::issue_credential_for_holder(&trust.issuer, &trust.authority, &holder)?;
    let authorization =
        credentials::holder_authorization_for_provider(&holder, &credential, &fman_pubkey)?;

    Ok((attestation, authorization, credential))
}
