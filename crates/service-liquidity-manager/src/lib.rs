//! FLIP Liquidity Manager service protocol.
//!
//! This crate translates the app-facing Public Liquidity API and the private
//! Operator Admin API into Rust request and response types plus service traits.

#![allow(async_fn_in_trait)]

mod admin;
mod canonical;
mod public;
mod service;
mod types;

pub use admin::*;
pub use canonical::*;
pub use fedi_credential_sdk_protocol::{
    CredentialDigest, CredentialsError, HolderAuthorization, HolderAuthorizationStatement,
    HolderId, IssuerAuthority, ProtocolV1, RevocationLocation, SchnorrSignatureProof,
    SignedCredential, SignedRevocation, SubjectPubkey, VerificationContext,
};
pub use fedi_decentralized_services::domain::{
    BitcoinNetwork, CanonicalPayload, FMAN_API_URLS_META_FIELD_KEY,
    FMAN_FEDERATION_TRUST_MATERIAL_SIGNATURE_DOMAIN_SEPARATOR,
    FMAN_PEER_ATTESTATION_SIGNATURE_DOMAIN_SEPARATOR, FMAN_SEAT_BINDINGS_META_FIELD_KEY,
    FMAN_TRUST_MATERIAL_MAX_RESPONSE_BYTES, FMAN_TRUST_MATERIAL_PEER_FILTER_MAX_COUNT,
    FederationId, FederationName, FederationSeat, FederationSeats, FleetSeatId,
    FmanApiUrlsMetadata, FmanFederationTrustMaterial, FmanFederationTrustMaterialVerificationError,
    FmanPeerAttestation, FmanPeerAttestationStatement, FmanSeatBindings, FmanSeatBindingsError,
    GatewayApiUrl, GetFederationTrustMaterialRequest, GetFederationTrustMaterialResponse,
    GuardianIdentity, HashBytes, HolderAuthorizationEnvelope, HolderTrustEnvelopeError, InviteCode,
    PayloadProof, PeerBadgeTrustPolicy, PeerBadgeTrustPolicyConfigError, PeerBadgeTrustPolicyError,
    PeerId, ProtocolVersion, Pubkey, Sats, SecretString, Signature, Signed, TRUST_SCORE_SCHEMA_V1,
    Timestamp, TrustScoreBadgeV1, TrustScoreSchemaError, Url, VerifiedSeatBinding,
    federation_seats, parse_trust_score_badge_v1, verify_holder_trust_envelope,
};
pub use fedi_decentralized_services::{ServiceError, ServiceErrorCode, ServiceResult};
pub use public::*;
pub use service::{
    OperatorAdminApi, PublicLiquidityApi, PublicLiquidityApiClient, PublicLiquidityApiServer,
};
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Serialize, de::DeserializeOwned};

    fn cbor_roundtrip<T>(value: &T) -> T
    where
        T: Serialize + DeserializeOwned + PartialEq + core::fmt::Debug,
    {
        let mut encoded = Vec::new();
        ciborium::into_writer(value, &mut encoded).expect("value serializes as CBOR");

        let decoded =
            ciborium::from_reader(encoded.as_slice()).expect("value deserializes from CBOR");
        assert_eq!(&decoded, value);
        decoded
    }

    fn cbor_as<T, U>(value: &T) -> U
    where
        T: Serialize,
        U: DeserializeOwned,
    {
        let mut encoded = Vec::new();
        ciborium::into_writer(value, &mut encoded).expect("value serializes as CBOR");

        ciborium::from_reader(encoded.as_slice()).expect("value deserializes from CBOR")
    }

    fn proof() -> PayloadProof {
        PayloadProof {
            signature: Signature(vec![1, 2, 3, 4]),
        }
    }

    fn sample_status() -> AllocationStatus {
        AllocationStatus {
            details_payload_hash: Sha256Digest([5; 32]),
            provider_pubkey: Pubkey("provider-pubkey".to_owned()),
            item_statuses: vec![AllocationItemStatus {
                target: AllocationItemTarget::Gateway {
                    item_id: ItemId("item-1".to_owned()),
                    gateway_id: GatewayId("gateway-1".to_owned()),
                    gateway_name: GatewayName("primary".to_owned()),
                    amount: Sats(42_000),
                },
                status: ItemAllocationStatus::Completed,
                fulfilled_amount: Some(Sats(42_000)),
                completion_evidence: Some(CompletionEvidence::Gateway(GatewayCompletionEvidence {
                    gateway_id: GatewayId("gateway-1".to_owned()),
                    gateway_api: GatewayApiUrl::try_from("https://gateway.example").unwrap(),
                    fulfilled_amount: Sats(42_000),
                    observed_gateway_balance: Sats(45_000),
                    observed_at: Timestamp(1_700_000_001),
                    withdrawal_txid: Some("txid".to_owned()),
                    wallet_operation_id: Some(WalletOperationId("wallet-op-1".to_owned())),
                })),
                failure: None,
                updated_at: Timestamp(1_700_000_002),
            }],
        }
    }

    fn sample_setup_config() -> SetupConfig {
        SetupConfig {
            network: BitcoinNetwork::Regtest,
            gateway: GatewayConfig {
                gateway_id: Some(GatewayId("gateway-1".to_owned())),
                gateway_name: GatewayName("primary".to_owned()),
                admin_url: "http://127.0.0.1:8175".to_owned(),
                identity_metadata: vec![("node_pubkey".to_owned(), "02abc".to_owned())],
            },
            chain_observer: ChainObserverConfig {
                backend: ChainObserverBackend::Esplora {
                    url: Url("http://127.0.0.1:3002".to_owned()),
                },
            },
            relays: vec![Url("wss://relay.example".to_owned())],
            capacity: CapacityConfig {
                mode: CapacityMode::AvailableFunds,
                explicit_cap: None,
                supported_sources: vec![SourceType::Gateway, SourceType::StabilityPool],
            },
            funding_policy: FundingPolicyConfig::defaults_for_network(BitcoinNetwork::Regtest),
            replenishment: ReplenishmentConfig {
                warning_threshold: Sats(10_000),
                critical_threshold: Sats(5_000),
            },
            advertised_endpoint: RpcEndpointConfig {
                endpoint_id: Some(RpcEndpointId("endpoint-1".to_owned())),
                transport: RpcTransport::Iroh,
                address: RpcEndpointAddress("iroh-node-id".to_owned()),
                discovery_hints: vec![RpcDiscoveryHint("relay:default".to_owned())],
                rpc_protocol_name: RpcProtocolName("fedi/flip/public-liquidity/1".to_owned()),
            },
            advertisement: AdvertisementConfig {
                republish_interval: DurationSecs(600),
                ready_advertisement_enabled: true,
            },
            provider_display: Some(ProviderDisplay {
                name: Some("Example FLIP".to_owned()),
                website: Some(Url("https://flip.example".to_owned())),
                contact: Some("ops@example".to_owned()),
            }),
            policy: ProviderPolicy {
                accepted_attester_policies: vec![AcceptedAttesterPolicy {
                    attester_pubkey: Pubkey("attester-pubkey".to_owned()),
                    verification_requirement: VerificationRequirement::AllTrusted,
                }],
                supported_networks: vec![BitcoinNetwork::Regtest],
            },
        }
    }

    fn sample_advertisement() -> LiquidityProviderAdvertisement {
        LiquidityProviderAdvertisement {
            version: ProtocolVersion(1),
            provider_pubkey: Pubkey("provider-pubkey".to_owned()),
            issued_at: Timestamp(1_700_000_000),
            expires_at: Timestamp(1_700_003_600),
            supported_sources: vec![SourceType::Gateway, SourceType::StabilityPool],
            holder_authorizations: vec![],
            policy: ProviderPolicy {
                accepted_attester_policies: vec![AcceptedAttesterPolicy {
                    attester_pubkey: Pubkey("attester-pubkey".to_owned()),
                    verification_requirement: VerificationRequirement::AllTrusted,
                }],
                supported_networks: vec![BitcoinNetwork::Regtest],
            },
            display: Some(ProviderDisplay {
                name: Some("Example FLIP".to_owned()),
                website: Some(Url("https://flip.example".to_owned())),
                contact: None,
            }),
            api_endpoints: vec![Url("iroh://provider-node".to_owned())],
            api_versions: vec![ProtocolVersion(1)],
            relay_hints: vec![Url("wss://relay.example".to_owned())],
        }
    }

    fn sample_liquidity_request() -> RequestLiquidityRequest {
        RequestLiquidityRequest {
            version: ProtocolVersion(1),
            requester_pubkey: Pubkey("requester-pubkey".to_owned()),
            provider_pubkey: Pubkey("provider-pubkey".to_owned()),
            issued_at: Timestamp(1_700_000_100),
            network: BitcoinNetwork::Regtest,
            amounts: LiquidityAmountBounds {
                gateway_min_amount: Sats(42_000),
                gateway_max_amount: None,
                stability_min_amount: Sats(0),
                stability_max_amount: None,
            },
            details_payload_hash: Sha256Digest([0; 32]),
            fman_endorsement: None,
            fman_trust_material: None,
            federation_details: FederationLiquidityDetails {
                invite_code: InviteCode("target-invite".to_owned()),
                federation_id: FederationId("federation-1".to_owned()),
                federation_name: FederationName("Target Federation".to_owned()),
                federation_config_hash: HashBytes(vec![1, 2, 3, 4]),
                fleet_seat_hints: vec![FleetSeat {
                    seat_id: FleetSeatId("seat-1".to_owned()),
                    peer_id: PeerId("0".to_owned()),
                    guardian_identity: GuardianIdentity("guardian-identity".to_owned()),
                    fleet_manager_pubkey: Pubkey("fman-pubkey".to_owned()),
                    role_metadata: vec![],
                }],
                revocation_locations: vec![],
            },
            expires_at: Timestamp(1_700_000_700),
        }
    }

    #[test]
    fn canonical_wire_display_strings_are_explicit() {
        assert_eq!(BitcoinNetwork::Bitcoin.to_string(), "bitcoin");
        assert_eq!(SourceType::StabilityPool.to_string(), "stability_pool");
        assert_eq!(
            VerificationRequirement::ConsensusMajorityTrusted.to_string(),
            "consensus_majority_trusted"
        );
        assert_eq!(HealthComponent::WebClient.to_string(), "web_client");
        assert_eq!(HealthComponent::ChainObserver.to_string(), "chain_observer");
        assert_eq!(
            PublicRejectionCode::InvalidAmountBounds.to_string(),
            "invalid_amount_bounds"
        );
        assert_eq!(
            AttestationKind::IssuerAuthority.to_string(),
            "issuer_authority"
        );
        assert_eq!(
            LiquidityFailureCode::InsufficientProviderFunds.to_string(),
            "insufficient_provider_funds"
        );
        assert_eq!(
            ItemAllocationStatus::ActionRequired.to_string(),
            "action_required"
        );
        assert_eq!(ItemAllocationStatus::Cancelled.to_string(), "cancelled");
        assert_eq!(WalletOperationStatus::Cancelled.to_string(), "cancelled");

        let rejection = PublicRejection {
            code: PublicRejectionCode::InternalError,
            reason: None,
        };
        assert_eq!(
            ProviderInfoOutcome::Rejected(rejection.clone()).to_string(),
            "rejected"
        );
        assert_eq!(
            RequestLiquidityOutcome::Rejected(rejection).to_string(),
            "rejected"
        );

        assert_eq!(
            AllocationItemTarget::StabilityPool {
                item_id: ItemId("item".to_owned()),
                amount: Sats(1),
            }
            .to_string(),
            "stability_pool"
        );
        assert_eq!(
            CompletionEvidence::Gateway(GatewayCompletionEvidence {
                gateway_id: GatewayId("gateway".to_owned()),
                gateway_api: GatewayApiUrl::try_from("https://gateway.example").unwrap(),
                fulfilled_amount: Sats(1),
                observed_gateway_balance: Sats(1),
                observed_at: Timestamp(0),
                withdrawal_txid: None,
                wallet_operation_id: None,
            })
            .to_string(),
            "gateway"
        );
    }

    #[test]
    fn funding_policy_defaults_are_network_specific() {
        let bitcoin = FundingPolicyConfig::defaults_for_network(BitcoinNetwork::Bitcoin);
        assert_eq!(bitcoin.fee_reserve, Sats(25_000));
        assert_eq!(bitcoin.confirmations, 3);
        assert_eq!(bitcoin.stability_pool_min_fee_rate_ppb, 0);

        let signet = FundingPolicyConfig::defaults_for_network(BitcoinNetwork::Signet);
        assert_eq!(signet.fee_reserve, Sats(5_000));
        assert_eq!(signet.confirmations, 1);

        let regtest = FundingPolicyConfig::defaults_for_network(BitcoinNetwork::Regtest);
        assert_eq!(regtest.fee_reserve, Sats(0));
        assert_eq!(regtest.confirmations, 1);
        assert_eq!(regtest.stability_pool_min_fee_rate_ppb, 0);
    }

    #[test]
    fn transport_wire_names_and_wrappers_are_stable() {
        let network: String = cbor_as(&BitcoinNetwork::Signet);
        assert_eq!(network, "signet");

        let health_component: String = cbor_as(&HealthComponent::PublicLiquidityApi);
        assert_eq!(health_component, "public_liquidity_api");

        let service_error_code: String = cbor_as(&ServiceErrorCode::PermissionDenied);
        assert_eq!(service_error_code, "permission_denied");

        let pubkey: String = cbor_as(&Pubkey("npub1example".to_owned()));
        assert_eq!(pubkey, "npub1example");

        let sats: u64 = cbor_as(&Sats(42));
        assert_eq!(sats, 42);
    }

    #[test]
    fn public_rpc_messages_round_trip_through_transport_cbor() {
        let request = Signed {
            payload: GetAllocationStatusRequest {
                version: ProtocolVersion(1),
                requester_pubkey: Pubkey("requester-pubkey".to_owned()),
                details_payload_hash: Sha256Digest([5; 32]),
                provider_pubkey: Pubkey("provider-pubkey".to_owned()),
                issued_at: Timestamp(1_700_000_000),
            },
            proof: proof(),
        };
        cbor_roundtrip(&request);

        let response = Signed {
            payload: GetAllocationStatusResponse {
                version: ProtocolVersion(1),
                provider_pubkey: Pubkey("provider-pubkey".to_owned()),
                issued_at: Timestamp(1_700_000_003),
                status: sample_status(),
            },
            proof: proof(),
        };
        cbor_roundtrip(&response);
    }

    #[test]
    fn admin_config_round_trips_through_transport_cbor() {
        let config = sample_setup_config();
        cbor_roundtrip(&config);

        let patch = UpdateProviderConfigRequest {
            patch: ProviderConfigPatch {
                funding_policy: Some(FundingPolicyConfig::defaults_for_network(
                    BitcoinNetwork::Signet,
                )),
                capacity: Some(CapacityConfig {
                    mode: CapacityMode::ExplicitCap,
                    explicit_cap: Some(Sats(100_000)),
                    supported_sources: vec![SourceType::Gateway],
                }),
                provider_display: Some(ProviderDisplayPatch::Clear),
                ..ProviderConfigPatch::default()
            },
        };
        cbor_roundtrip(&patch);
    }

    #[test]
    fn provider_display_patch_round_trips_explicit_json_actions() {
        let omitted: ProviderConfigPatch =
            serde_json::from_value(serde_json::json!({})).expect("omitted field deserializes");
        assert_eq!(omitted.provider_display, None);
        let explicit_null: ProviderConfigPatch =
            serde_json::from_value(serde_json::json!({ "provider_display": null }))
                .expect("null field deserializes");
        assert_eq!(explicit_null.provider_display, None);

        let display = ProviderDisplay {
            name: Some("Example FLIP".to_owned()),
            website: Some(Url("https://flip.example".to_owned())),
            contact: Some("ops@example".to_owned()),
        };

        let set = ProviderConfigPatch {
            provider_display: Some(ProviderDisplayPatch::Set(display.clone())),
            ..ProviderConfigPatch::default()
        };
        let set_json = serde_json::to_value(&set).expect("set patch serializes");
        assert_eq!(
            set_json
                .get("provider_display")
                .expect("provider display field is serialized"),
            &serde_json::json!({
                "action": "set",
                "value": display.clone(),
            })
        );
        let decoded_set: ProviderConfigPatch =
            serde_json::from_value(set_json).expect("set patch deserializes");
        assert_eq!(
            decoded_set.provider_display,
            Some(ProviderDisplayPatch::Set(display))
        );

        let clear = ProviderConfigPatch {
            provider_display: Some(ProviderDisplayPatch::Clear),
            ..ProviderConfigPatch::default()
        };
        let clear_json = serde_json::to_value(&clear).expect("clear patch serializes");
        assert_eq!(
            clear_json
                .get("provider_display")
                .expect("provider display field is serialized"),
            &serde_json::json!({ "action": "clear" })
        );
        let decoded_clear: ProviderConfigPatch =
            serde_json::from_value(clear_json).expect("clear patch deserializes");
        assert_eq!(
            decoded_clear.provider_display,
            Some(ProviderDisplayPatch::Clear)
        );
    }

    #[test]
    fn service_errors_round_trip_through_transport_cbor() {
        let result: ServiceResult<Signed<GetAllocationStatusResponse>> = Err(
            ServiceError::with_code(ServiceErrorCode::NotFound, "allocation not found"),
        );
        let decoded = cbor_roundtrip(&result);

        let err = decoded.expect_err("result remains an error after transport decode");
        assert_eq!(err.code(), ServiceErrorCode::NotFound);
        assert_eq!(err.message(), "allocation not found");
    }

    #[test]
    fn canonical_json_payload_is_independent_of_key_order() {
        let left = serde_json::json!({
            "z": true,
            "nested": {
                "b": 2,
                "a": 1
            },
            "a": "first"
        });
        let right = serde_json::json!({
            "a": "first",
            "nested": {
                "a": 1,
                "b": 2
            },
            "z": true
        });

        let left_payload = canonical_json_payload(&left).expect("left payload canonicalizes");
        let right_payload = canonical_json_payload(&right).expect("right payload canonicalizes");

        assert_eq!(left_payload, right_payload);
        assert_eq!(
            domain_tagged_sha256(
                LIQUIDITY_PROVIDER_ADVERTISEMENT_SIGNATURE_DOMAIN,
                &left_payload.0
            ),
            domain_tagged_sha256(
                LIQUIDITY_PROVIDER_ADVERTISEMENT_SIGNATURE_DOMAIN,
                &right_payload.0
            )
        );
    }

    #[test]
    fn advertisement_hash_uses_advertisement_domain() {
        let advertisement = sample_advertisement();
        let canonical =
            advertisement_canonical_payload(&advertisement).expect("advertisement canonicalizes");
        let hash = advertisement_hash(&advertisement).expect("advertisement hashes");

        assert_eq!(
            hash,
            domain_tagged_sha256(
                LIQUIDITY_PROVIDER_ADVERTISEMENT_SIGNATURE_DOMAIN,
                &canonical.0
            )
        );
    }

    #[test]
    fn rpc_payload_hashes_are_domain_separated() {
        let request = GetAllocationStatusRequest {
            version: ProtocolVersion(1),
            requester_pubkey: Pubkey("requester-pubkey".to_owned()),
            details_payload_hash: Sha256Digest([5; 32]),
            provider_pubkey: Pubkey("provider-pubkey".to_owned()),
            issued_at: Timestamp(1_700_000_000),
        };
        let canonical = public_rpc_canonical_payload(&request).expect("RPC payload canonicalizes");

        let correct =
            public_rpc_payload_hash(PublicRpcPayloadDomain::GetAllocationStatusRequest, &request)
                .expect("RPC payload hashes");
        let wrong_domain = domain_tagged_sha256(
            GET_ALLOCATION_STATUS_RESPONSE_SIGNATURE_DOMAIN,
            &canonical.0,
        );

        assert_ne!(correct, wrong_domain);
    }

    #[test]
    fn details_payload_hash_uses_commitment_fields_only() {
        let mut request = sample_liquidity_request();
        let original =
            request_liquidity_details_hash_for_request(&request).expect("request hashes");

        request.issued_at = Timestamp(request.issued_at.0 + 1);
        request.details_payload_hash = Sha256Digest([99; 32]);
        let changed_freshness_only =
            request_liquidity_details_hash_for_request(&request).expect("request hashes");
        assert_eq!(original, changed_freshness_only);

        // Trust material is collected per attempt, so a retry carrying freshly
        // fetched material must resolve to the same allocation rather than
        // read as a conflicting request.
        request.fman_trust_material = Some(Vec::new());
        let changed_trust_material_only =
            request_liquidity_details_hash_for_request(&request).expect("request hashes");
        assert_eq!(original, changed_trust_material_only);

        request.federation_details.federation_name =
            FederationName("Different Federation".to_owned());
        let changed_commitment =
            request_liquidity_details_hash_for_request(&request).expect("request hashes");
        assert_ne!(original, changed_commitment);
    }

    #[test]
    fn provider_display_validation_enforces_limits() {
        let valid = ProviderDisplay {
            name: Some("n".repeat(PROVIDER_DISPLAY_NAME_MAX_BYTES)),
            website: Some(Url(format!(
                "https://{}",
                "w".repeat(PROVIDER_DISPLAY_WEBSITE_MAX_BYTES - "https://".len())
            ))),
            contact: Some("c".repeat(PROVIDER_DISPLAY_CONTACT_MAX_BYTES)),
        };
        assert_eq!(valid.validate(), Ok(()));

        let empty = ProviderDisplay {
            name: None,
            website: None,
            contact: None,
        };
        assert_eq!(empty.validate(), Ok(()));

        let long_name = ProviderDisplay {
            name: Some("n".repeat(PROVIDER_DISPLAY_NAME_MAX_BYTES + 1)),
            website: None,
            contact: None,
        };
        assert_eq!(
            long_name.validate(),
            Err(ProviderDisplayValidationError::NameTooLong)
        );

        let long_website = ProviderDisplay {
            name: None,
            website: Some(Url(format!(
                "https://{}",
                "w".repeat(PROVIDER_DISPLAY_WEBSITE_MAX_BYTES)
            ))),
            contact: None,
        };
        assert_eq!(
            long_website.validate(),
            Err(ProviderDisplayValidationError::WebsiteTooLong)
        );

        let http_website = ProviderDisplay {
            name: None,
            website: Some(Url("http://flip.example".to_owned())),
            contact: None,
        };
        assert_eq!(
            http_website.validate(),
            Err(ProviderDisplayValidationError::WebsiteNotHttps)
        );

        let long_contact = ProviderDisplay {
            name: None,
            website: None,
            contact: Some("c".repeat(PROVIDER_DISPLAY_CONTACT_MAX_BYTES + 1)),
        };
        assert_eq!(
            long_contact.validate(),
            Err(ProviderDisplayValidationError::ContactTooLong)
        );

        let control_name = ProviderDisplay {
            name: Some("bad\u{0007}name".to_owned()),
            website: None,
            contact: None,
        };
        assert_eq!(
            control_name.validate(),
            Err(ProviderDisplayValidationError::ControlCharacter)
        );
    }

    #[test]
    fn legacy_json_with_credential_locations_still_deserializes() {
        // Persisted config rows and admin payloads written before the inline
        // trust-carriage adoption may still carry a `credential_locations`
        // key; serde must ignore it instead of failing to load.
        let mut config_json = serde_json::to_value(sample_setup_config()).expect("serializes");
        config_json
            .as_object_mut()
            .expect("config is an object")
            .insert(
                "credential_locations".to_owned(),
                serde_json::json!(["nostr:provider-creds"]),
            );
        let config: SetupConfig = serde_json::from_value(config_json).expect("legacy key ignored");
        assert_eq!(config, sample_setup_config());

        let mut patch_json = serde_json::to_value(ProviderConfigPatch::default()).expect("patch");
        patch_json
            .as_object_mut()
            .expect("patch is an object")
            .insert(
                "credential_locations".to_owned(),
                serde_json::json!(["nostr:provider-creds"]),
            );
        let patch: ProviderConfigPatch =
            serde_json::from_value(patch_json).expect("legacy key ignored");
        assert_eq!(patch, ProviderConfigPatch::default());
    }
}
