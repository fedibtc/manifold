//! Fixed representative values for the FMan contract-fixture JSON committed
//! under `operator-ui/packages/types/fixtures/`.
//!
//! This module is shared, via `#[path]`, by the generator binary
//! (`src/bin/gen_fman_contract_fixtures.rs`) and the drift test
//! (`tests/contract_fixtures.rs`), so both sides build the exact same values
//! from one definition. It lives under `tests/support/` (not a top-level
//! `tests/*.rs` file) so cargo does not treat it as its own test binary.
//!
//! Two things separate this from the liquidity-manager module it mirrors
//! (`crates/service-liquidity-manager/tests/support/fixtures.rs`):
//!
//! - Almost nothing on the FMan admin surface is a serde-derived response
//!   struct. `admin.rs` hand-encodes the JSON, so every response fixture here
//!   is produced by calling that module's own `*_json` shaper. A fixture that
//!   rebuilt the shapes locally would be a second copy of the contract, which
//!   is the drift this wave exists to remove.
//! - The request side is covered exhaustively rather than sampled. The walk in
//!   [`request_fixtures`] is driven by a `match` over `AdminRequest`, so adding
//!   a verb stops this crate compiling until the verb is in the fixture set.

#![allow(dead_code)]

use bitcoin::secp256k1::{PublicKey, SECP256K1, SecretKey};
use fedi_decentralized_domain::FmanVersion;
use fedi_decentralized_service_fleet_manager::{
    FederationId, FiId, InviteCode, Plan, QuoteId, SeatHealth, SeatId,
};
use fedimint_core::Amount;
use fman_core::admin::{self, AdminError, AdminErrorKind, AdminRequest};
use fman_core::backup_worker::BackupScanOutcome;
use fman_core::directory::{DirectoryPresence, OnboardingStatus};
use fman_core::facts::{CompletionCallbackReason, CompletionCallbackStatus};
use fman_core::fleet::PaymentFederationStatus;
use fman_core::guardian_fee::{
    Collected, CollectionFailure, CollectionFailurePhase, FederationFeeStatus, FeePolicy,
    Remittance,
};
use fman_core::onboarding;
use fman_core::payout_wire::{
    DrainStateWire, OutgoingOperationWire, OutgoingRailWire, OutgoingStateWire,
    PayoutJobOperationWire, PayoutJobStatusWire, PayoutJobWire, PayoutScopeWire,
    WalletDrainStatusWire,
};
use fman_core::remittance_metadata::{RemittanceBreakdownItem, RemittanceMetadata};
use fman_core::seat::{PaymentClaimStatus, SeatBackupStatus, SeatPhase, SeatReport, SeatSummary};
use fman_core::wallet::PayoutRequestId;
use serde_json::Value;
use stability_pool_client::common::{Account, AccountType};

/// File stem of the request-inventory fixture. It is the one fixture the
/// TypeScript mock catalogue is checked against, so it is named separately.
pub const REQUESTS_FIXTURE: &str = "fman_admin_requests";

/// File stem of the error-kind inventory. Covered exhaustively for the same
/// reason the requests are: a consumer branches on these, so one appearing
/// without the TypeScript union hearing about it is a silent lie.
pub const ERROR_KINDS_FIXTURE: &str = "fman_admin_error_kinds";

/// The fixture names this module produces, in generation order.
pub const FIXTURE_NAMES: &[&str] = &[
    REQUESTS_FIXTURE,
    ERROR_KINDS_FIXTURE,
    "fman_admin_error",
    "fman_plans",
    "fman_payment_federations",
    "fman_payout_destination",
    "fman_payout_job",
    "fman_payout_job_status",
    "fman_seats",
    "fman_seat_reports",
    "fman_seat_guardian_fees",
    "fman_seat_status",
    "fman_decommission_seat",
    "fman_reenroll_telemetry",
    "fman_guardian_fees",
    "fman_collect_guardian_fees",
    "fman_collect_guardian_fees_incomplete_idle",
    "fman_collect_guardian_fees_incomplete",
    "fman_collect_guardian_fees_incomplete_refresh",
    "fman_onboarding",
    "fman_holder_authorization_refresh",
    "fman_mnemonic",
    "fman_onboard_as_new",
    "fman_onboard_as_new_already",
    "fman_onboard_from_backup",
];

/// Pretty-printed JSON for every fixture, paired with its file stem. Every
/// value comes back through `crate::admin`'s own shapers, so the committed
/// files are what the daemon writes on the wire, not a description of it.
pub fn fixture_json() -> Vec<(&'static str, String)> {
    let pairs: Vec<(&'static str, Value)> = vec![
        (REQUESTS_FIXTURE, requests_fixture()),
        (ERROR_KINDS_FIXTURE, error_kinds_fixture()),
        ("fman_admin_error", admin_error_fixture()),
        ("fman_plans", plans_fixture()),
        ("fman_payment_federations", payment_federations_fixture()),
        ("fman_payout_destination", payout_destination_fixture()),
        ("fman_payout_job", payout_job_fixture()),
        ("fman_payout_job_status", payout_job_status_fixture()),
        ("fman_seats", seats_fixture()),
        ("fman_seat_reports", seat_reports_fixture()),
        ("fman_seat_guardian_fees", seat_guardian_fees_fixture()),
        ("fman_seat_status", seat_status_fixture()),
        ("fman_decommission_seat", decommission_seat_fixture()),
        ("fman_reenroll_telemetry", reenroll_telemetry_fixture()),
        ("fman_guardian_fees", guardian_fees_fixture()),
        (
            "fman_collect_guardian_fees",
            collect_guardian_fees_fixture(),
        ),
        (
            "fman_collect_guardian_fees_incomplete_idle",
            collect_guardian_fees_incomplete_idle_fixture(),
        ),
        (
            "fman_collect_guardian_fees_incomplete",
            collect_guardian_fees_incomplete_fixture(),
        ),
        (
            "fman_collect_guardian_fees_incomplete_refresh",
            collect_guardian_fees_incomplete_refresh_fixture(),
        ),
        ("fman_onboarding", onboarding_fixture()),
        ("fman_holder_authorization_refresh", onboarding_fixture()),
        ("fman_mnemonic", mnemonic_fixture()),
        ("fman_onboard_as_new", onboarding::onboarded_new_json()),
        (
            "fman_onboard_as_new_already",
            admin::onboarded_already_json(),
        ),
        (
            "fman_onboard_from_backup",
            onboarding::onboarded_restored_json(2, 1),
        ),
    ];
    pairs
        .into_iter()
        .map(|(name, value)| (name, serde_json::to_string_pretty(&value).unwrap()))
        .collect()
}

// --- fixed identities and amounts -------------------------------------------
//
// Every value below is a constant: no clocks, no randomness, so regenerating
// the fixtures produces a byte-identical file until a shape actually changes.

/// A seat id is 32 fixed bytes rendered as 64 hex characters.
pub fn seat_id() -> SeatId {
    SeatId::from(QuoteId([7; 32]))
}

fn other_seat_id() -> SeatId {
    SeatId::from(QuoteId([8; 32]))
}

/// secp256k1's generator x-coordinate — a known-valid x-only key that needs no
/// key generation.
fn fi_id() -> FiId {
    FiId(
        "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
            .parse()
            .expect("fixed x-only key is valid"),
    )
}

fn nostr_pubkey(hex: &str) -> nostr_sdk::PublicKey {
    hex.parse().expect("fixed nostr key is valid")
}

fn service_nostr_pubkey() -> nostr_sdk::PublicKey {
    nostr_pubkey("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798")
}

fn holder_nostr_pubkey() -> nostr_sdk::PublicKey {
    nostr_pubkey("c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5")
}

/// The remittance account exactly as `Fleet::guardian_fee_account` reports it:
/// a stability-pool `Account` serialized to a JSON *string*, so the wire value
/// is JSON nested inside a JSON string.
fn remittance_account() -> String {
    let key = PublicKey::from_secret_key(
        SECP256K1,
        &SecretKey::from_slice(&[0x11; 32]).expect("fixed scalar is valid"),
    );
    serde_json::to_string(&Account::single(key, AccountType::BtcDepositor))
        .expect("account serializes")
}

fn plan() -> Plan {
    Plan::InfiniteBestEffort {
        price_msats: 50_000_000,
    }
}

fn fee_policy() -> FeePolicy {
    FeePolicy {
        configured: true,
        send_ppm: Some(1_000),
        recipients: Some(
            r#"{"version":1,"recipients":[{"account_id":"fixture","weight":1}]}"#.to_owned(),
        ),
        our_share: Some((1, 4)),
    }
}

// --- requests ---------------------------------------------------------------

/// Every `AdminRequest` variant, keyed by its externally-tagged name.
///
/// The TypeScript mirror asserts one entry per declared request variant, and
/// the MSW verb catalogue is checked against these keys, so a verb the daemon
/// dropped cannot keep being answered by the mock.
pub fn requests_fixture() -> Value {
    let mut map = serde_json::Map::new();
    for request in request_fixtures() {
        map.insert(
            request_name(&request).to_owned(),
            serde_json::to_value(&request).expect("AdminRequest serializes"),
        );
    }
    Value::Object(map)
}

/// Every `AdminErrorKind`, as the token it serializes to, in declaration
/// order.
///
/// Walked the same way the requests are, and for the same reason: the walk is
/// driven by a `match`, so adding a kind stops this module compiling until the
/// kind is in the fixture, and the TypeScript union is checked against the
/// file.
pub fn error_kinds_fixture() -> Value {
    Value::Array(
        error_kinds()
            .into_iter()
            .map(|kind| serde_json::to_value(kind).expect("AdminErrorKind serializes"))
            .collect(),
    )
}

/// One representative error envelope, so the `{ kind, message }` shape is
/// covered and not only the token set.
pub fn admin_error_fixture() -> Value {
    serde_json::to_value(AdminError {
        kind: AdminErrorKind::SeatDirectoryExists,
        message: "seat 0707070707070707070707070707070707070707070707070707070707070707 would be \
                  restored over an existing seat directory"
            .to_owned(),
    })
    .expect("AdminError serializes")
}

pub fn error_kinds() -> Vec<AdminErrorKind> {
    let mut all = vec![AdminErrorKind::UnparsableRequest];
    while let Some(next) = kind_after(*all.last().expect("seeded with the first variant")) {
        assert!(
            all.len() < 100,
            "the AdminErrorKind walk revisits a variant; `kind_after` must form a chain"
        );
        all.push(next);
    }
    all
}

/// The next kind in the walk. Exhaustive on purpose — see [`error_kinds`].
fn kind_after(kind: AdminErrorKind) -> Option<AdminErrorKind> {
    Some(match kind {
        AdminErrorKind::UnparsableRequest => AdminErrorKind::NotOnboarded,
        AdminErrorKind::NotOnboarded => AdminErrorKind::AlreadyOnboarded,
        AdminErrorKind::AlreadyOnboarded => AdminErrorKind::InvalidMnemonic,
        AdminErrorKind::InvalidMnemonic => AdminErrorKind::RestoreNotAcknowledged,
        AdminErrorKind::RestoreNotAcknowledged => AdminErrorKind::UnreadableBackupDocument,
        AdminErrorKind::UnreadableBackupDocument => AdminErrorKind::SeatDirectoryExists,
        AdminErrorKind::SeatDirectoryExists => AdminErrorKind::MissingGuardianArchive,
        AdminErrorKind::MissingGuardianArchive => AdminErrorKind::Other,
        AdminErrorKind::Other => return None,
    })
}

/// One representative value per `AdminRequest` variant.
///
/// The list is *walked*, not written out: [`after`] is exhaustive over the
/// enum, so a new variant makes this module stop compiling until it is
/// threaded into the walk — which is what puts it in the fixture set. A
/// hand-kept list would keep passing while silently missing the new verb.
pub fn request_fixtures() -> Vec<AdminRequest> {
    let mut all = vec![AdminRequest::ShowPlans];
    while let Some(next) = after(all.last().expect("seeded with the first variant")) {
        assert!(
            all.len() < 100,
            "the AdminRequest walk revisits a variant; `after` must form a chain, not a cycle"
        );
        all.push(next);
    }
    all
}

/// The next variant in the walk, with the representative value it is covered
/// by. Exhaustive on purpose — see [`request_fixtures`].
fn after(request: &AdminRequest) -> Option<AdminRequest> {
    let seat_id = seat_id();
    Some(match request {
        AdminRequest::ShowPlans => AdminRequest::SetPrice {
            price_msats: Some(50_000_000),
        },
        AdminRequest::SetPrice { .. } => AdminRequest::ShowCapacity,
        AdminRequest::ShowCapacity => AdminRequest::SetCapacity { max_seats: 4 },
        AdminRequest::SetCapacity { .. } => AdminRequest::ListPaymentFederations,
        AdminRequest::ListPaymentFederations => AdminRequest::PayoutDestination,
        AdminRequest::PayoutDestination => AdminRequest::SetPayoutDestination {
            destination: Some("operator@example.com".to_owned()),
        },
        AdminRequest::SetPayoutDestination { .. } => AdminRequest::SweepPaymentFees {
            federation_id: FederationId("fed1fixturepaymentfederation".to_owned()),
            request_id: "fixture-payment-payout".parse().unwrap(),
        },
        AdminRequest::SweepPaymentFees { .. } => AdminRequest::PayoutStatus {
            request_id: "fixture-payment-payout".parse().unwrap(),
        },
        AdminRequest::PayoutStatus { .. } => AdminRequest::AwaitPayout {
            request_id: "fixture-payment-payout".parse().unwrap(),
        },
        AdminRequest::AwaitPayout { .. } => AdminRequest::ListSeats,
        AdminRequest::ListSeats => AdminRequest::SeatStatus {
            seat_id: seat_id.clone(),
        },
        AdminRequest::SeatStatus { .. } => AdminRequest::DecommissionSeat {
            seat_id: seat_id.clone(),
        },
        AdminRequest::DecommissionSeat { .. } => AdminRequest::ReenrollTelemetry,
        AdminRequest::ReenrollTelemetry => AdminRequest::GuardianFees {
            seat_id: seat_id.clone(),
            limit: Some(20),
        },
        AdminRequest::GuardianFees { .. } => AdminRequest::CollectGuardianFees {
            seat_id: seat_id.clone(),
        },
        AdminRequest::CollectGuardianFees { .. } => AdminRequest::SweepGuardianFees {
            seat_id,
            request_id: "fixture-guardian-payout".parse().unwrap(),
        },
        AdminRequest::SweepGuardianFees { .. } => AdminRequest::Onboarding,
        AdminRequest::Onboarding => AdminRequest::RefreshHolderAuthorizations,
        AdminRequest::RefreshHolderAuthorizations => AdminRequest::ConfigureInitialOffer {
            max_seats: 4,
            price_msats: Some(50_000_000),
        },
        AdminRequest::ConfigureInitialOffer { .. } => AdminRequest::ShowMnemonic,
        AdminRequest::ShowMnemonic => AdminRequest::OnboardAsNew { if_needed: true },
        AdminRequest::OnboardAsNew { .. } => AdminRequest::OnboardFromBackup {
            mnemonic: MNEMONIC.to_owned(),
            acknowledge_original_host_is_gone: true,
        },
        AdminRequest::OnboardFromBackup { .. } => return None,
    })
}

/// The externally-tagged name serde gives each variant. Exhaustive for the
/// same reason [`after`] is.
pub fn request_name(request: &AdminRequest) -> &'static str {
    match request {
        AdminRequest::ShowPlans => "ShowPlans",
        AdminRequest::SetPrice { .. } => "SetPrice",
        AdminRequest::ShowCapacity => "ShowCapacity",
        AdminRequest::SetCapacity { .. } => "SetCapacity",
        AdminRequest::ListPaymentFederations => "ListPaymentFederations",
        AdminRequest::PayoutDestination => "PayoutDestination",
        AdminRequest::SetPayoutDestination { .. } => "SetPayoutDestination",
        AdminRequest::SweepPaymentFees { .. } => "SweepPaymentFees",
        AdminRequest::PayoutStatus { .. } => "PayoutStatus",
        AdminRequest::AwaitPayout { .. } => "AwaitPayout",
        AdminRequest::ListSeats => "ListSeats",
        AdminRequest::SeatStatus { .. } => "SeatStatus",
        AdminRequest::DecommissionSeat { .. } => "DecommissionSeat",
        AdminRequest::ReenrollTelemetry => "ReenrollTelemetry",
        AdminRequest::GuardianFees { .. } => "GuardianFees",
        AdminRequest::CollectGuardianFees { .. } => "CollectGuardianFees",
        AdminRequest::SweepGuardianFees { .. } => "SweepGuardianFees",
        AdminRequest::Onboarding => "Onboarding",
        AdminRequest::RefreshHolderAuthorizations => "RefreshHolderAuthorizations",
        AdminRequest::ConfigureInitialOffer { .. } => "ConfigureInitialOffer",
        AdminRequest::ShowMnemonic => "ShowMnemonic",
        AdminRequest::OnboardAsNew { .. } => "OnboardAsNew",
        AdminRequest::OnboardFromBackup { .. } => "OnboardFromBackup",
    }
}

/// The all-`abandon` BIP-39 test vector. Not a phrase any real fleet holds.
const MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

// --- responses --------------------------------------------------------------

pub fn plans_fixture() -> Value {
    admin::plans_json(vec![plan()])
}

/// Covers all three states an operator sees: an accepted federation with a
/// balance, an accepted one whose wallet cannot answer (null, never zero), and
/// a wallet-only leftover of a removed member.
pub fn payment_federations_fixture() -> Value {
    admin::payment_federations_json(vec![
        PaymentFederationStatus {
            federation_id: FederationId("fed1fixtureacceptedfederation".into()),
            accepted: true,
            receivable: true,
            wallet: WalletDrainStatusWire {
                available_ecash_msat: Some(350),
                economically_sweepable_recipient_msat: Some(0),
                encumbered_outgoing_msat: None,
                outgoing: Some(vec![OutgoingOperationWire {
                    operation_id:
                        "17d55b3cb3e9cd25035f6b8cf296284d4445ba9ea8568ccf5ab198d4df27a5ce".into(),
                    rail: OutgoingRailWire::Lnv1,
                    state: OutgoingStateWire::Pending,
                    recipient_amount_msat: 3_960_152,
                    contract_amount_msat: 3_980_000,
                    encumbered_msat: None,
                    has_active_state_machines: true,
                }]),
                active_operation_count: 1,
                query_errors: vec![],
                drain_state: DrainStateWire::PendingWalletWork,
            },
        },
        PaymentFederationStatus {
            federation_id: FederationId("fed1fixtureunreadablefederation".into()),
            accepted: true,
            receivable: false,
            wallet: unavailable_wallet(),
        },
        PaymentFederationStatus {
            federation_id: FederationId("fed1fixtureleftoverfederation".into()),
            accepted: false,
            receivable: false,
            wallet: drained_wallet(0, 0),
        },
    ])
}
fn unavailable_wallet() -> WalletDrainStatusWire {
    WalletDrainStatusWire::unavailable()
}
fn drained_wallet(available: u64, sweepable: u64) -> WalletDrainStatusWire {
    WalletDrainStatusWire {
        available_ecash_msat: Some(available),
        economically_sweepable_recipient_msat: Some(sweepable),
        encumbered_outgoing_msat: Some(0),
        outgoing: Some(vec![]),
        active_operation_count: 0,
        query_errors: vec![],
        drain_state: if sweepable > 0 {
            DrainStateWire::Sweepable
        } else {
            DrainStateWire::Drained
        },
    }
}

pub fn payout_destination_fixture() -> Value {
    admin::payout_destination_json(Some("operator@example.com".to_owned()))
}

pub fn payout_job() -> PayoutJobWire {
    PayoutJobWire {
        request_id: PayoutRequestId::parse("fixture-payout-request").unwrap(),
        scope: PayoutScopeWire::PaymentFederation {
            federation_id: FederationId("fed1fixturepayment".into()),
        },
        destination: "operator@example.com".into(),
        operation: Some(PayoutJobOperationWire {
            operation_id: "0f7c1b9a3e5d4c2b8a6f0e1d2c3b4a5960718293a4b5c6d7e8f90a1b2c3d4e5f".into(),
            amount_msat: 250_000,
            committed_at_ms: 1_753_600_002_000,
        }),
        created_at_ms: 1_753_600_001_000,
    }
}
pub fn payout_job_fixture() -> Value {
    serde_json::to_value(payout_job()).unwrap()
}
pub fn payout_job_status_fixture() -> Value {
    serde_json::to_value(PayoutJobStatusWire {
        job: payout_job(),
        payout: Some(OutgoingOperationWire {
            operation_id: "0f7c1b9a3e5d4c2b8a6f0e1d2c3b4a5960718293a4b5c6d7e8f90a1b2c3d4e5f".into(),
            rail: OutgoingRailWire::Lnv2,
            state: OutgoingStateWire::Succeeded,
            recipient_amount_msat: 250_000,
            contract_amount_msat: 251_000,
            encumbered_msat: Some(0),
            has_active_state_machines: false,
        }),
    })
    .unwrap()
}

/// One seat per `PaymentClaimStatus`, per `CompletionCallbackStatus`, and per
/// backup shape (unconfirmed, document-only, document-plus-archive), so a
/// change to any hand-encoded enum shows up in the committed JSON.
pub fn seats_fixture() -> Value {
    admin::seats_json(
        vec![
            seat_summary(
                PaymentClaimStatus::NotPaid,
                CompletionCallbackStatus::NotConfigured,
                false,
                None,
            ),
            seat_summary(
                PaymentClaimStatus::Pending,
                CompletionCallbackStatus::Pending {
                    attempts: 2,
                    next_attempt_at_ms: 1_753_600_060_000,
                    last_reason: Some(CompletionCallbackReason::Network),
                },
                false,
                Some(SeatBackupStatus {
                    published_at_ms: 1_753_600_010_000,
                    archive_confirmed: false,
                }),
            ),
            seat_summary(
                PaymentClaimStatus::Success {
                    at_ms: 1_753_600_000_000,
                },
                CompletionCallbackStatus::Delivered {
                    attempts: 1,
                    at_ms: 1_753_600_030_000,
                },
                false,
                Some(SeatBackupStatus {
                    published_at_ms: 1_753_600_020_000,
                    archive_confirmed: true,
                }),
            ),
            seat_summary(
                PaymentClaimStatus::AlreadySpent {
                    at_ms: 1_753_599_000_000,
                },
                CompletionCallbackStatus::OperatorBlocked {
                    attempts: 3,
                    reason: CompletionCallbackReason::GatewayOriginMissing,
                },
                false,
                None,
            ),
            seat_summary(
                PaymentClaimStatus::NotPaid,
                CompletionCallbackStatus::Terminal {
                    attempts: 5,
                    at_ms: 1_753_500_000_000,
                    reason: CompletionCallbackReason::Decommissioned,
                },
                true,
                Some(SeatBackupStatus {
                    published_at_ms: 1_753_600_040_000,
                    archive_confirmed: true,
                }),
            ),
        ],
        Some(BackupScanOutcome {
            completed_at_ms: 1_753_600_050_000,
            pending_seats: 1,
        }),
    )
}

fn seat_summary(
    payment_claim: PaymentClaimStatus,
    completion_callback: CompletionCallbackStatus,
    decommissioned: bool,
    backup: Option<SeatBackupStatus>,
) -> SeatSummary {
    SeatSummary {
        seat_id: if decommissioned {
            other_seat_id()
        } else {
            seat_id()
        },
        fi_id: fi_id(),
        plan: plan(),
        created_at_ms: 1_753_500_000_000,
        payment_claim,
        decommissioned,
        completion_callback,
        backup,
    }
}

/// Every `SeatReport` shape: the decommissioned terminal, and each `SeatPhase`
/// an active seat passes through, across the three `SeatHealth` values.
pub fn seat_reports_fixture() -> Value {
    let reports = vec![
        SeatReport::Decommissioned {
            at_ms: 1_753_500_000_000,
        },
        SeatReport::Active {
            phase: SeatPhase::Created,
            health: SeatHealth::Healthy,
        },
        SeatReport::Active {
            phase: SeatPhase::DkgInProgress,
            health: SeatHealth::Failed,
        },
        SeatReport::Active {
            phase: SeatPhase::Running {
                invite_code: InviteCode("fed11fixtureinvitecode".to_owned()),
            },
            health: SeatHealth::Healthy,
        },
        SeatReport::Active {
            phase: SeatPhase::DataLoss {
                invite_code: InviteCode("fed11fixturelostinvite".to_owned()),
            },
            health: SeatHealth::Unavailable,
        },
    ];
    Value::Array(reports.into_iter().map(admin::report_json).collect())
}

/// All three shapes `SeatStatus.guardian_fee` can take: no derivable account,
/// an account whose policy read failed, and a fully-read policy.
pub fn seat_guardian_fees_fixture() -> Value {
    Value::Array(vec![
        admin::seat_guardian_fee_error_json("guardian-fee collection is unavailable"),
        admin::seat_guardian_fee_json(
            remittance_account(),
            Err(anyhow::anyhow!("seat has no federation yet")),
        ),
        admin::seat_guardian_fee_json(remittance_account(), Ok(fee_policy())),
    ])
}

pub fn seat_status_fixture() -> Value {
    admin::seat_status_json(
        seat_summary(
            PaymentClaimStatus::Success {
                at_ms: 1_753_600_000_000,
            },
            CompletionCallbackStatus::Delivered {
                attempts: 1,
                at_ms: 1_753_600_030_000,
            },
            false,
            Some(SeatBackupStatus {
                published_at_ms: 1_753_600_020_000,
                archive_confirmed: true,
            }),
        ),
        SeatReport::Active {
            phase: SeatPhase::Running {
                invite_code: InviteCode("fed11fixtureinvitecode".to_owned()),
            },
            health: SeatHealth::Healthy,
        },
        admin::seat_guardian_fee_json(remittance_account(), Ok(fee_policy())),
    )
}

pub fn decommission_seat_fixture() -> Value {
    admin::decommission_seat_json(true)
}

pub fn reenroll_telemetry_fixture() -> Value {
    admin::reenroll_telemetry_json()
}

/// Both remittance shapes: a breakdown that opened, and one whose sealed
/// paperwork did not — the amount is reported either way.
pub fn guardian_fees_fixture() -> Value {
    admin::guardian_fees_json(
        &seat_id(),
        &FederationFeeStatus {
            federation_id: fedimint_core::config::FederationId::dummy(),
            account_id: Account::single(
                PublicKey::from_secret_key(
                    SECP256K1,
                    &SecretKey::from_slice(&[0x11; 32]).expect("fixed scalar is valid"),
                ),
                AccountType::BtcDepositor,
            )
            .id(),
            staged: Amount::from_msats(1_500_000),
            locked: Amount::from_msats(500_000),
            idle: Amount::from_msats(250_000),
            history_count: 2,
        },
        remittance_account(),
        drained_wallet(8_000_000, 7_950_000),
        // Deliberately larger than the two remittances below add up to: the
        // lifetime total is not a sum of the window, and a fixture where the
        // two agreed would let a consumer that totals the window pass.
        41_500_000,
        &fee_policy(),
        vec![
            Remittance {
                amount: Amount::from_msats(1_200_000),
                txid: "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90".to_owned(),
                metadata: Ok(RemittanceMetadata {
                    version: 1,
                    total_msats: 4_800_000,
                    breakdown: vec![
                        RemittanceBreakdownItem {
                            module: "ln".to_owned(),
                            direction: "outgoing".to_owned(),
                            amount_msats: 3_000_000,
                        },
                        RemittanceBreakdownItem {
                            module: "mint".to_owned(),
                            direction: "incoming".to_owned(),
                            amount_msats: 1_800_000,
                        },
                    ],
                    remitted_at_unix: 1_753_600_000,
                }),
            },
            Remittance {
                amount: Amount::from_msats(300_000),
                txid: "b1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90".to_owned(),
                metadata: Err("sealed breakdown could not be opened".to_owned()),
            },
        ],
    )
}

pub fn collect_guardian_fees_fixture() -> Value {
    admin::collect_guardian_fees_json(Collected::Complete {
        claimed: Amount::from_msats(1_750_000),
        awaiting_cycle: Amount::from_msats(500_000),
    })
}

pub fn collect_guardian_fees_incomplete_fixture() -> Value {
    admin::collect_guardian_fees_json(Collected::Incomplete {
        confirmed_claimed: Amount::from_msats(1_750_000),
        observed_awaiting_cycle: None,
        failure: CollectionFailure {
            phase: CollectionFailurePhase::Unlock,
            operation_submitted: false,
        },
    })
}

pub fn collect_guardian_fees_incomplete_idle_fixture() -> Value {
    admin::collect_guardian_fees_json(Collected::Incomplete {
        confirmed_claimed: Amount::ZERO,
        observed_awaiting_cycle: Some(Amount::from_msats(500_000)),
        failure: CollectionFailure {
            phase: CollectionFailurePhase::IdleClaim,
            operation_submitted: true,
        },
    })
}

pub fn collect_guardian_fees_incomplete_refresh_fixture() -> Value {
    admin::collect_guardian_fees_json(Collected::Incomplete {
        confirmed_claimed: Amount::from_msats(1_750_000),
        observed_awaiting_cycle: None,
        failure: CollectionFailure {
            phase: CollectionFailurePhase::BalanceRefresh,
            operation_submitted: false,
        },
    })
}

/// The authorized state, with an update available — the shape that carries
/// every optional field the waiting state omits.
pub fn onboarding_fixture() -> Value {
    admin::onboarding_json(
        "02a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f9",
        &DirectoryPresence {
            service_nostr_pubkey: service_nostr_pubkey(),
            onboarding: OnboardingStatus::AuthorizationObserved {
                authorizations: 1,
                holders: vec![holder_nostr_pubkey()],
                checked_at: Some(1_760_000_000),
            },
            latest_fman_version: Some(fman_version("0.2.0")),
        },
        &fman_version("0.1.0"),
    )
}

fn fman_version(version: &str) -> FmanVersion {
    version.parse().expect("fixed version is valid SemVer")
}

pub fn mnemonic_fixture() -> Value {
    admin::mnemonic_json(MNEMONIC)
}
