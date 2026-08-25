//! Issue test-only PeerBadge material from the public development/staging roots.

use std::time::Duration;

use anyhow::{Context as _, ensure};
use clap::Parser;
use fedi_credential_sdk_protocol::{
    HolderAuthorizationRequest, HolderContext, IssuerAuthority, IssuerContext, IssuerSecretKeys,
    PendingIssuance,
};
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_nostr::attester::{
    ISSUER_AUTHORITY_D_TAG, ISSUER_AUTHORITY_EVENT_KIND, ISSUER_AUTHORITY_HASHTAG,
    NOSTR_REVOCATION_LOCATION_PROTOCOL,
};
use fedi_decentralized_nostr_clients::{
    HolderNostrClient as _, NostrHolderClient, NostrRelayClient, PublishFlipAuthorizationRequest,
    PublishFmanAuthorizationRequest,
};
use nostr_sdk::{EventBuilder, Keys, Kind, SecretKey, Tag};
use serde_json::json;

const TRUST_BADGE_SCHEMA: &str = "fedi-trust-score-v1.0";
const RELAY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Parser)]
#[command(about = "Issue trust material from Manifold's unsafe test-only issuer roots")]
struct Args {
    /// Manifold environment (`development` or `staging`; production is refused).
    #[arg(long)]
    environment: ManifoldEnvironment,

    /// Publish to this relay instead of the environment's canonical relays.
    /// Test and development harnesses use this with an isolated local relay.
    #[arg(long)]
    relay: Option<String>,

    /// Print a freshly signed `IssuerAuthority` document for the committed
    /// issuer secret (revocation locations = the profile's canonical relays)
    /// and exit without issuing or publishing anything. Fixture maintenance
    /// only: the output is what belongs at
    /// `crates/manifold-environment/fixtures/<env>-issuer-authority.json`
    /// whenever the committed secret or the canonical relays change.
    #[arg(long, conflicts_with = "authorization_request")]
    mint_authority_document: bool,

    /// JSON-encoded SDK HolderAuthorizationRequest, matching the QR/deep-link payload.
    #[arg(long, required_unless_present = "mint_authority_document")]
    authorization_request: Option<String>,

    /// Publish the holder authorization event that an FMan discovers.
    #[arg(long)]
    publish_fman_authorization: bool,

    /// Publish the holder authorization event that a FLIP provider discovers.
    ///
    /// The same authorization as the FMan variant, with the same event kind and
    /// the same content — only the discovery coordinate differs. FLIP fetches
    /// with `.hashtag(FLIP_AUTHORIZATION_HASHTAG)`, so an FMan-tagged event is
    /// never a candidate for it however correctly it names the provider in `p`.
    #[arg(long)]
    publish_flip_authorization: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args = Args::parse();
    ensure!(
        args.environment != ManifoldEnvironment::Production,
        "the test issuer refuses the production environment"
    );
    let profile = args.environment.profile()?;
    // The complete issuer secret — identity and PBRSA issuance key — is
    // committed in the environment profile, so every run of this tool signs
    // under the one canonical authority instead of minting a per-machine
    // issuance key and rotating the replaceable kind-37703 event out from
    // under previously issued credentials. Production commits no secret and
    // fails here (in addition to the explicit refusal above).
    let committed = profile
        .test_issuer_secret_keys()
        .context("the selected environment commits no test issuer secret")?;
    let secret: IssuerSecretKeys =
        serde_json::from_str(committed).context("parse committed issuer secret")?;
    let issuer =
        IssuerContext::import_secret_key(&secret).context("import committed issuer secret")?;
    let issuer_keys = Keys::parse(&secret.issuer_id_secret_key)?;
    ensure!(
        profile
            .peer_badge_issuer_identities()
            .contains(&issuer_keys.public_key()),
        "committed test issuer identity is not a configured environment root"
    );

    let relay_urls = args.relay.clone().map_or_else(
        || {
            profile
                .nostr_relays()
                .as_urls()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        },
        |relay| vec![relay],
    );

    if args.mint_authority_document {
        let minted = issuer.issuer_authority(
            relay_urls
                .iter()
                .map(
                    |location| fedi_credential_sdk_protocol::RevocationLocation {
                        protocol: NOSTR_REVOCATION_LOCATION_PROTOCOL.to_owned(),
                        location: location.clone(),
                    },
                )
                .collect(),
        )?;
        println!("{}", serde_json::to_string(&minted)?);
        return Ok(());
    }

    // Publish the committed authority document verbatim: verifiers pin this
    // exact document, so the relay copy and the pinned copy cannot diverge.
    let authority: IssuerAuthority = profile
        .pinned_issuer_authorities()
        .iter()
        .find_map(|document| {
            serde_json::from_str::<IssuerAuthority>(document)
                .ok()
                .filter(|authority| authority.issuer.issuer_id_pubkey.0 == issuer_keys.public_key())
        })
        .context("the environment pins no authority document for the committed issuer")?;
    let issuer_metadata = authority.verify()?;
    let authorization_request: HolderAuthorizationRequest = serde_json::from_str(
        args.authorization_request
            .as_deref()
            .expect("clap requires --authorization-request unless minting"),
    )
    .context("parse --authorization-request as HolderAuthorizationRequest JSON")?;
    let subject = authorization_request.subject_pubkey.clone();

    let holder = HolderContext::generate();
    let holder_pubkey = holder.public_key().to_string();
    let credential_info =
        fedi_credential_sdk_schemas::trust_score_info_v1(profile.minimum_peer_badge_trust_level())?;
    let (issuance_request, pending) = PendingIssuance::create_request(
        &issuer_metadata.issuance_key,
        issuer_metadata.issuer_id_pubkey.clone(),
        credential_info.clone(),
        json!(holder_pubkey),
    )?;
    let response = issuer.issue_credential(credential_info, &issuance_request)?;
    let credential = pending.finalize(&issuer_metadata.issuance_key, &response)?;
    let authorization = holder.authorize_credential_use(authorization_request, &credential)?;
    let credential_digest = serde_json::to_value(&authorization.authorization.credential_digest)?
        .as_str()
        .context("credential digest must serialize as a string")?
        .to_owned();

    let mut authority_event_ids = Vec::new();
    let mut authorization_event_ids = Vec::new();
    for relay_url in &relay_urls {
        let issuer_relay =
            NostrRelayClient::connect(relay_url, issuer_keys.clone(), RELAY_TIMEOUT).await?;
        let event_id = issuer_relay
            .publish_event(
                EventBuilder::new(
                    Kind::Custom(ISSUER_AUTHORITY_EVENT_KIND),
                    serde_json::to_string(&authority)?,
                )
                .tags([
                    Tag::identifier(ISSUER_AUTHORITY_D_TAG),
                    Tag::hashtag(ISSUER_AUTHORITY_HASHTAG),
                ]),
            )
            .await?;
        authority_event_ids.push(event_id.to_string());

        if args.publish_fman_authorization || args.publish_flip_authorization {
            let holder_keys = Keys::new(SecretKey::parse(&holder.export_secret_key())?);
            let holder_client = NostrHolderClient::new(
                NostrRelayClient::connect(relay_url, holder_keys, RELAY_TIMEOUT).await?,
            );
            // One envelope for both coordinates: the components differ in how
            // they index the authorization, never in what it says.
            let envelope = json!({
                "version": 1,
                "holder_id_pubkey": holder.public_key(),
                "holder_authorization": authorization,
                "signed_credential": credential,
            });
            let content = serde_json_canonicalizer::to_string(&envelope)?;

            if args.publish_fman_authorization {
                let event_id = holder_client
                    .publish_fman_authorization(PublishFmanAuthorizationRequest {
                        fman_pubkey: subject.0,
                        issuer_pubkey: issuer_keys.public_key().to_string(),
                        credential_digest: credential_digest.clone(),
                        schema: TRUST_BADGE_SCHEMA.to_owned(),
                        content: content.clone(),
                    })
                    .await?;
                authorization_event_ids.push(event_id.to_string());
            }

            if args.publish_flip_authorization {
                let event_id = holder_client
                    .publish_flip_authorization(PublishFlipAuthorizationRequest {
                        provider_pubkey: subject.0,
                        credential_digest: credential_digest.clone(),
                        content,
                    })
                    .await?;
                authorization_event_ids.push(event_id.to_string());
            }
        }
    }

    println!(
        "{}",
        serde_json::to_string(&json!({
            "environment": args.environment.to_string(),
            "subject_pubkey": subject.0.to_string(),
            "issuer_pubkey": issuer_keys.public_key(),
            "holder_pubkey": holder.public_key(),
            "issuer_authority": authority,
            "signed_credential": credential,
            "holder_authorization": authorization,
            "authority_event_ids": authority_event_ids,
            "authorization_event_ids": authorization_event_ids,
        }))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_secrets_match_environment_profiles() {
        for environment in [
            ManifoldEnvironment::Development,
            ManifoldEnvironment::Staging,
        ] {
            let profile = environment.profile().unwrap();
            let secret: IssuerSecretKeys =
                serde_json::from_str(profile.test_issuer_secret_keys().unwrap()).unwrap();
            let keys = Keys::parse(&secret.issuer_id_secret_key).unwrap();
            assert_eq!(profile.peer_badge_issuer_identities(), &[keys.public_key()]);
        }
    }

    #[test]
    fn production_has_no_test_issuer_secret() {
        assert!(
            ManifoldEnvironment::Production
                .profile()
                .unwrap()
                .test_issuer_secret_keys()
                .is_none()
        );
    }

    #[test]
    fn authorization_request_uses_the_sdk_json_contract() {
        let request: HolderAuthorizationRequest = serde_json::from_str(
            r#"{"subject_pubkey":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"}"#,
        )
        .unwrap();
        assert_eq!(
            request.subject_pubkey.0.to_string(),
            "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        );
    }
}
