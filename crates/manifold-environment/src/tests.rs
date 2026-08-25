use nostr::nips::nip19::ToBech32;

use super::*;

#[test]
fn development_reuses_staging_relay_but_not_issuer_root() {
    let development = ManifoldEnvironment::Development
        .profile_with_env(|_| None)
        .unwrap();
    let staging = ManifoldEnvironment::Staging
        .profile_with_env(|_| None)
        .unwrap();
    assert_eq!(development.nostr_relays(), staging.nostr_relays());
    assert_ne!(
        development.peer_badge_issuer_identities(),
        staging.peer_badge_issuer_identities()
    );
    assert_eq!(
        staging.nostr_relays().as_urls()[0].to_string(),
        "wss://relay-staging.dev.fedibtc.com"
    );
}

#[test]
fn every_environment_requires_the_trusted_peer_badge_tier() {
    for environment in [
        ManifoldEnvironment::Development,
        ManifoldEnvironment::Staging,
        ManifoldEnvironment::Production,
    ] {
        assert_eq!(
            environment
                .profile_with_env(|_| None)
                .unwrap()
                .minimum_peer_badge_trust_level(),
            9,
        );
    }
}

#[test]
fn environments_pin_their_bitcoin_network_and_public_default_backend() {
    let development = ManifoldEnvironment::Development
        .profile_with_env(|_| None)
        .unwrap();
    let staging = ManifoldEnvironment::Staging
        .profile_with_env(|_| None)
        .unwrap();
    let production = ManifoldEnvironment::Production
        .profile_with_env(|_| None)
        .unwrap();

    assert_eq!(development.bitcoin_network(), Network::Regtest);
    assert_eq!(development.default_esplora_url(), None);
    assert_eq!(staging.bitcoin_network(), Network::Signet);
    assert_eq!(
        staging.default_esplora_url().map(Url::as_str),
        Some("https://mutinynet.com/api/")
    );
    assert_eq!(production.bitcoin_network(), Network::Bitcoin);
    assert_eq!(production.default_esplora_url(), None);
}

#[test]
fn development_and_staging_have_distinct_placeholder_setup_payment_publishers() {
    let development = ManifoldEnvironment::Development
        .profile_with_env(|_| None)
        .unwrap();
    let staging = ManifoldEnvironment::Staging
        .profile_with_env(|_| None)
        .unwrap();
    let development_publisher = *development.setup_payment_publisher().unwrap();
    let staging_publisher = *staging.setup_payment_publisher().unwrap();
    assert_ne!(development_publisher, staging_publisher);
    let issuer_identity_union = development
        .peer_badge_issuer_identities()
        .iter()
        .chain(staging.peer_badge_issuer_identities())
        .copied()
        .collect::<Vec<_>>();
    for (profile, publisher) in [
        (&development, development_publisher),
        (&staging, staging_publisher),
    ] {
        assert!(
            !issuer_identity_union.contains(&publisher),
            "{} publisher placeholder must stay distinct from every issuer placeholder",
            profile.environment()
        );
    }
}

#[test]
fn development_and_staging_pin_distinct_full_fedi_fee_accounts() {
    let development = ManifoldEnvironment::Development
        .profile_with_env(|_| None)
        .unwrap();
    let staging = ManifoldEnvironment::Staging
        .profile_with_env(|_| None)
        .unwrap();
    let development_account = development.fedi_guardian_fee_account().unwrap();
    let staging_account = staging.fedi_guardian_fee_account().unwrap();

    assert_ne!(development_account, staging_account);
    for account in [development_account, staging_account] {
        assert_eq!(account.acc_type(), AccountType::BtcDepositor);
        assert!(account.as_single().is_some());
    }
    assert_eq!(
        development_account.as_single().unwrap().to_string(),
        DEVELOPMENT_PLACEHOLDER_FEDI_GUARDIAN_FEE_PUBLIC_KEY,
    );
    assert_eq!(
        staging_account.as_single().unwrap().to_string(),
        STAGING_PLACEHOLDER_FEDI_GUARDIAN_FEE_PUBLIC_KEY,
    );
}

#[test]
fn production_profile_uses_fedi_app_production_relays() {
    let production = ManifoldEnvironment::Production
        .profile_with_env(|_| None)
        .unwrap();
    let relay_urls = production
        .nostr_relays()
        .as_urls()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert_eq!(
        relay_urls,
        &[
            "wss://relay.dev.fedibtc.com",
            "wss://relay.primal.net",
            "wss://relay.damus.io/",
        ]
    );
    assert!(production.setup_payment_publisher().is_none());
    assert!(production.fedi_guardian_fee_account().is_none());
}

#[test]
fn production_pins_its_issuer_identities() {
    let production = ManifoldEnvironment::Production
        .profile_with_env(|_| None)
        .unwrap();
    let issuers = production
        .peer_badge_issuer_identities()
        .iter()
        .map(|issuer| issuer.to_bech32().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        issuers,
        &[
            "npub14jty0h6e4kjugvkw0lu6dta6c3g862wd7mkea8wjvqgwvs0lnt4s528jdn",
            "npub1mvml033envht57j7cr8cykx92eqxh7ypmv673lg0u9yew405s59s4qxa89",
            "npub13n05t087zy939r6ulu64kfxtsh65neuaapu65d5324c0m7d4pvpqx4psp0",
            "npub17mrmcy3wwjkw7cgp6t358flx3gx78kwunu6hc7f8vapmsmlak2ps06v0cw",
            "npub1jm4nvzweed7q0k5ztep607nnul5qyryen3w7qzenxdrkckxcgrjq2d9xz9",
        ]
    );

    // Individually held production keys must never collide with the
    // known-secret development or staging placeholders.
    let mut placeholders = [
        DEVELOPMENT_PLACEHOLDER_ISSUER,
        STAGING_PLACEHOLDER_ISSUER,
        DEVELOPMENT_PLACEHOLDER_SETUP_PAYMENT_PUBLISHER,
        STAGING_PLACEHOLDER_SETUP_PAYMENT_PUBLISHER,
    ]
    .map(|placeholder| PublicKey::parse(placeholder).unwrap())
    .to_vec();
    placeholders.extend(
        [
            DEVELOPMENT_PLACEHOLDER_FEDI_GUARDIAN_FEE_PUBLIC_KEY,
            STAGING_PLACEHOLDER_FEDI_GUARDIAN_FEE_PUBLIC_KEY,
        ]
        .map(|placeholder| {
            let compressed = placeholder
                .parse::<bitcoin::secp256k1::PublicKey>()
                .unwrap();
            PublicKey::from_slice(&compressed.x_only_public_key().0.serialize()).unwrap()
        }),
    );
    for issuer in production.peer_badge_issuer_identities() {
        assert!(
            !placeholders.contains(issuer),
            "production issuer must not be a known-secret placeholder"
        );
    }
}

#[test]
fn development_overrides_replace_relays_and_publisher() {
    let env = |variable: &'static str| match variable {
        DEV_NOSTR_RELAYS_ENV => {
            Some("wss://one.example, wss://two.example wss://one.example".to_owned())
        }
        DEV_SETUP_PAYMENT_PUBLISHER_ENV => {
            Some(STAGING_PLACEHOLDER_SETUP_PAYMENT_PUBLISHER.to_owned())
        }
        _ => None,
    };
    let profile = ManifoldEnvironment::Development
        .profile_with_env(env)
        .unwrap();
    let relay_urls = profile
        .nostr_relays()
        .as_urls()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert_eq!(relay_urls, &["wss://one.example", "wss://two.example"]);
    assert_eq!(
        profile.setup_payment_publisher(),
        Some(&PublicKey::parse(STAGING_PLACEHOLDER_SETUP_PAYMENT_PUBLISHER).unwrap())
    );
    // Issuer identities are not overridable.
    assert_eq!(
        profile.peer_badge_issuer_identities(),
        ManifoldEnvironment::Development
            .profile_with_env(|_| None)
            .unwrap()
            .peer_badge_issuer_identities()
    );
}

#[test]
fn staging_and_production_refuse_development_overrides() {
    for environment in [
        ManifoldEnvironment::Staging,
        ManifoldEnvironment::Production,
    ] {
        for variable in [DEV_NOSTR_RELAYS_ENV, DEV_SETUP_PAYMENT_PUBLISHER_ENV] {
            let err = environment
                .profile_with_env(|name| (name == variable).then(|| "anything".to_owned()))
                .unwrap_err();
            assert_eq!(
                err,
                ManifoldEnvironmentProfileError::OverrideOutsideDevelopment {
                    variable,
                    environment,
                }
            );
        }
    }
}

#[test]
fn malformed_development_overrides_are_refused() {
    for (variable, value) in [
        (DEV_NOSTR_RELAYS_ENV, "not a url"),
        (DEV_NOSTR_RELAYS_ENV, " , "),
        (DEV_SETUP_PAYMENT_PUBLISHER_ENV, "not-a-key"),
    ] {
        let err = ManifoldEnvironment::Development
            .profile_with_env(|name| (name == variable).then(|| value.to_owned()))
            .unwrap_err();
        assert!(matches!(
            err,
            ManifoldEnvironmentProfileError::InvalidOverride { variable: failed, .. }
                if failed == variable
        ));
    }
}

#[test]
fn environment_aliases_round_trip_to_canonical_names() {
    for (input, expected, display) in [
        ("dev", ManifoldEnvironment::Development, "development"),
        ("staging", ManifoldEnvironment::Staging, "staging"),
        ("prod", ManifoldEnvironment::Production, "production"),
    ] {
        assert_eq!(input.parse::<ManifoldEnvironment>().unwrap(), expected);
        assert_eq!(expected.to_string(), display);
        assert_eq!(display.parse::<ManifoldEnvironment>().unwrap(), expected);
        assert_eq!(
            expected.profile_with_env(|_| None).unwrap().environment(),
            expected
        );
        assert_eq!(
            expected
                .profile_with_env(|_| None)
                .unwrap()
                .profile_revision(),
            7
        );
    }
    assert!("nightly".parse::<ManifoldEnvironment>().is_err());
}

/// The committed secret and the committed authority document must describe
/// one issuer: same placeholder identity, same PBRSA issuance key, revocation
/// routed at the profile's canonical relays — and a credential issued with
/// the secret must verify against the pinned document. This is the agreement
/// that keeps the test issuer (signing with the secret, publishing the
/// document) and verifiers (pinning the document) on one canonical authority.
#[test]
fn committed_issuer_material_is_one_agreeing_authority() {
    use fedi_credential_sdk_protocol::{
        HolderAuthorizationRequest, HolderContext, IssuerAuthority, IssuerContext,
        IssuerSecretKeys, PendingIssuance, SubjectPubkey, VerificationContext,
    };

    for (environment, placeholder) in [
        (
            ManifoldEnvironment::Development,
            DEVELOPMENT_PLACEHOLDER_ISSUER,
        ),
        (ManifoldEnvironment::Staging, STAGING_PLACEHOLDER_ISSUER),
    ] {
        let profile = environment.profile_with_env(|_| None).unwrap();
        let secret: IssuerSecretKeys = serde_json::from_str(
            profile
                .test_issuer_secret_keys()
                .expect("known-secret environments commit their issuer secret"),
        )
        .expect("committed issuer secret parses");
        let issuer =
            IssuerContext::import_secret_key(&secret).expect("committed issuer secret imports");

        let documents = profile.pinned_issuer_authorities();
        assert_eq!(documents.len(), 1, "{environment} pins one authority");
        let authority: IssuerAuthority =
            serde_json::from_str(documents[0]).expect("committed authority document parses");
        let issuer_metadata = authority
            .verify()
            .expect("committed authority document verifies");

        assert_eq!(
            issuer_metadata.issuer_id_pubkey.0.to_string(),
            placeholder,
            "{environment} authority must belong to the placeholder issuer",
        );
        let relays: Vec<String> = profile
            .nostr_relays()
            .as_urls()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            issuer_metadata
                .revocation
                .iter()
                .map(|location| (location.protocol.as_str(), location.location.clone()))
                .collect::<Vec<_>>(),
            relays
                .iter()
                .map(|relay| ("nostr", relay.clone()))
                .collect::<Vec<_>>(),
            "{environment} authority routes revocation at the canonical relays",
        );

        // Full round trip: a credential issued with the committed secret must
        // verify against the pinned document, proving secret and document
        // carry the same issuance key.
        let holder = HolderContext::generate();
        let info = fedi_credential_sdk_schemas::trust_score_info_v1(
            profile.minimum_peer_badge_trust_level(),
        )
        .expect("trust score info");
        let (request, pending) = PendingIssuance::create_request(
            &issuer_metadata.issuance_key,
            issuer_metadata.issuer_id_pubkey.clone(),
            info.clone(),
            serde_json::json!(holder.public_key().to_string()),
        )
        .expect("create issuance request");
        let response = issuer
            .issue_credential(info, &request)
            .expect("issue credential with committed secret");
        let credential = pending
            .finalize(&issuer_metadata.issuance_key, &response)
            .expect("finalize credential");
        let authorization = holder
            .authorize_credential_use(
                HolderAuthorizationRequest {
                    subject_pubkey: placeholder
                        .parse::<SubjectPubkey>()
                        .expect("placeholder parses as a subject"),
                },
                &credential,
            )
            .expect("authorize credential");
        let mut verifier = VerificationContext::new();
        verifier
            .add_issuer_authority(&authority)
            .expect("pin committed authority");
        verifier
            .verify_credential_authorization(&credential, &authorization)
            .expect(
                "credential issued with the committed secret verifies against the pinned document",
            );
    }
}

#[test]
fn production_commits_no_issuer_material() {
    let profile = ManifoldEnvironment::Production
        .profile_with_env(|_| None)
        .unwrap();
    assert_eq!(profile.test_issuer_secret_keys(), None);
    assert!(profile.pinned_issuer_authorities().is_empty());
}
