//! Fixed representative values for the contract-fixture JSON committed under
//! `operator-ui/packages/types/fixtures/`.
//!
//! This module is shared, via `#[path]`, by the generator binary
//! (`src/bin/gen_contract_fixtures.rs`) and the drift test
//! (`tests/contract_fixtures.rs`), so both sides build the exact same values
//! from one definition. It lives under `tests/support/` (not a top-level
//! `tests/*.rs` file) so cargo does not treat it as its own test binary.

#![allow(dead_code)]

use fedi_decentralized_service_liquidity_manager::*;

/// The fixture names this module produces, in generation order.
pub const FIXTURE_NAMES: &[&str] = &[
    "health",
    "funds",
    "advertisement",
    "attestations",
    "backup_manifest",
    "paging",
];

/// Pretty-printed JSON for every fixture, paired with its file stem, each
/// serialized directly by its response type's own serde impl (no
/// intermediate `Value`, so field order matches the struct declaration
/// exactly as the daemon would emit it).
pub fn fixture_json() -> Vec<(&'static str, String)> {
    vec![
        (
            "health",
            serde_json::to_string_pretty(&health_fixture()).unwrap(),
        ),
        (
            "funds",
            serde_json::to_string_pretty(&funds_fixture()).unwrap(),
        ),
        (
            "advertisement",
            serde_json::to_string_pretty(&advertisement_fixture()).unwrap(),
        ),
        (
            "attestations",
            serde_json::to_string_pretty(&attestations_fixture()).unwrap(),
        ),
        (
            "backup_manifest",
            serde_json::to_string_pretty(&backup_manifest_fixture()).unwrap(),
        ),
        (
            "paging",
            serde_json::to_string_pretty(&paging_fixture()).unwrap(),
        ),
    ]
}

// Canonical fully-healthy snapshot — the mock world's `healthyHealth` scenario
// derives directly from this fixture (see packages/mock-fixtures/src/health.ts).
pub fn health_fixture() -> GetHealthResponse {
    GetHealthResponse {
        overall_status: HealthStatus::Healthy,
        mode: HealthMode::Normal,
        observed_at: Timestamp(1721476800),
        components: vec![
            ComponentHealth {
                component: HealthComponent::Daemon,
                status: HealthStatus::Healthy,
                detail: None,
                observed_at: Timestamp(1721476800),
            },
            ComponentHealth {
                component: HealthComponent::Wallet,
                status: HealthStatus::Healthy,
                detail: None,
                observed_at: Timestamp(1721476800),
            },
            ComponentHealth {
                component: HealthComponent::Gateway,
                status: HealthStatus::Healthy,
                detail: None,
                observed_at: Timestamp(1721476800),
            },
            ComponentHealth {
                component: HealthComponent::ChainObserver,
                status: HealthStatus::Healthy,
                detail: None,
                observed_at: Timestamp(1721476800),
            },
        ],
    }
}

pub fn funds_fixture() -> GetFundsResponse {
    GetFundsResponse {
        balance: WalletBalanceSummary {
            spendable: Sats(4_200_000),
            pending_incoming: Sats(150_000),
            pending_outgoing: Sats(50_000),
            in_flight_allocations: Sats(800_000),
            fee_reserve: Sats(150_000),
            available_balance: Sats(3_250_000),
        },
        replenishment: ReplenishmentStatus::Ok,
        gateway: GatewayInventoryState {
            gateway_id: GatewayId("gw-signet-01".to_string()),
            gateway_name: GatewayName("Mock Signet Gateway".to_string()),
            status: InventoryStatus::Available,
            available_amount: Sats(3_000_000),
            observed_at: Some(Timestamp(1721476800)),
        },
        stability_pool: StabilityPoolInventoryState {
            status: InventoryStatus::Available,
            available_amount: Sats(250_000),
            observed_at: Some(Timestamp(1721476800)),
        },
        effective_liquidity: vec![
            EffectiveLiquidityItem {
                source_type: SourceType::Gateway,
                gateway_id: Some(GatewayId("gw-signet-01".to_string())),
                amount: Sats(3_000_000),
            },
            EffectiveLiquidityItem {
                source_type: SourceType::StabilityPool,
                gateway_id: None,
                amount: Sats(250_000),
            },
        ],
    }
}

pub fn advertisement_fixture() -> GetAdvertisementStateResponse {
    let provider_pubkey = Pubkey("02aa".to_string() + &"0".repeat(62));
    let relay_url = Url("wss://relay.signet.example".to_string());

    GetAdvertisementStateResponse {
        advertisement: Some(Signed {
            payload: LiquidityProviderAdvertisement {
                version: ProtocolVersion(1),
                provider_pubkey: provider_pubkey.clone(),
                issued_at: Timestamp(1784505600),
                expires_at: Timestamp(1784509200),
                supported_sources: vec![SourceType::Gateway, SourceType::StabilityPool],
                holder_authorizations: vec![],
                policy: ProviderPolicy {
                    accepted_attester_policies: vec![AcceptedAttesterPolicy {
                        attester_pubkey: provider_pubkey,
                        verification_requirement: VerificationRequirement::AllTrusted,
                    }],
                    supported_networks: vec![BitcoinNetwork::Signet],
                },
                display: Some(ProviderDisplay {
                    name: Some("Mock FLIP".to_string()),
                    website: Some(Url("https://flip.example".to_string())),
                    contact: Some("ops@flip.example".to_string()),
                }),
                api_endpoints: vec![Url("https://flip.example/api".to_string())],
                api_versions: vec![ProtocolVersion(1)],
                relay_hints: vec![relay_url.clone()],
            },
            proof: PayloadProof {
                signature: Signature(vec![1, 2, 3, 4]),
            },
        }),
        publication_status: AdvertisementPublicationStatus::Published,
        last_published_at: Some(Timestamp(1784505600)),
        expires_at: Some(Timestamp(1784509200)),
        withdrawn_at: None,
        relay_states: vec![RelayPublicationState {
            relay_url,
            status: RelayStatus::Published,
            last_error: None,
            last_seen_at: Some(Timestamp(1784505600)),
        }],
        ready: true,
        readiness: None,
        unverified_holder_authorization_count: 0,
    }
}

pub fn attestations_fixture() -> AttestationListResponse {
    let issuer_pubkey = Pubkey("03bb".to_string() + &"0".repeat(62));

    AttestationListResponse {
        payloads: vec![AttestationPayloadInfo {
            id: AttestationPayloadId("att-issuer-authority-01".to_string()),
            kind: AttestationKind::IssuerAuthority,
            issuer: Some(issuer_pubkey.clone()),
            subject: AttestationSubject::Issuer(issuer_pubkey),
            ingested_at: Timestamp(1784538000),
            valid: true,
        }],
    }
}

pub fn backup_manifest_fixture() -> BackupManifest {
    BackupManifest {
        version: ProtocolVersion(3),
        created_at: Timestamp(1721476800),
        state_groups: vec![
            BackupStateGroup::ProviderIdentity,
            BackupStateGroup::Attestations,
            BackupStateGroup::WalletClientState,
            BackupStateGroup::Database,
            BackupStateGroup::OperationHistory,
            BackupStateGroup::OperatorConfig,
            BackupStateGroup::ExternalDependencies,
        ],
        recovery_point: BackupRecoveryPoint {
            quiesced_at: Timestamp(1721476790),
            stores: vec![BackupStore::Sqlite, BackupStore::DataDirectory],
        },
    }
}

pub fn paging_fixture() -> ListWalletOperationsResponse {
    ListWalletOperationsResponse {
        operations: ListResponse {
            items: vec![
                WalletOperationSummary {
                    operation_id: WalletOperationId("wop-0003".to_string()),
                    operation_type: WalletOperationType::Deposit,
                    amount: Sats(1_000_000),
                    status: WalletOperationStatus::Confirmed,
                    federation_id: None,
                    created_at: Timestamp(1721476800),
                    updated_at: Timestamp(1721477100),
                },
                WalletOperationSummary {
                    operation_id: WalletOperationId("wop-0002".to_string()),
                    operation_type: WalletOperationType::GatewayFunding,
                    amount: Sats(500_000),
                    status: WalletOperationStatus::Completed,
                    federation_id: Some(FederationId("fed-gw-01".to_string())),
                    created_at: Timestamp(1721390400),
                    updated_at: Timestamp(1721390700),
                },
            ],
            next_page: Some(PageCursor("wop-0001".to_string())),
        },
    }
}
