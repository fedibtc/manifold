// Validates the Rust-generated contract fixtures under ../../fixtures/.
//
// TypeScript types vanish at runtime, so a bare `JSON.parse` + cast proves
// nothing, and a JSON module import alone isn't enough either: `tsc` widens
// every JSON leaf to its primitive type (e.g. a literal `"healthy"` becomes
// plain `string`), so `satisfies HealthStatus` would pass for *any* string,
// not just the real enum values. To get a real compile-time check we mirror
// each fixture as a genuine TypeScript object literal — contextual typing
// then keeps the literal union types intact — and a runtime test asserts the
// mirror is byte-for-byte what the generator actually produced. Together:
//   - compile-time: `satisfies <ResponseType>` on the literal mirror, so
//     `tsc` rejects real shape/enum drift (it fails immediately against
//     `Timestamp = string`, or against a component status not in
//     `HealthStatus`).
//   - runtime: the mirror is asserted equal to the committed JSON, so the
//     mirror itself cannot silently drift from what `just
//     gen-contract-fixtures` produces; plus direct assertions on the JSON
//     for the invariants the brief calls out (numeric timestamps, exactly
//     the 7 real backup state groups).
//
// Regenerate the fixtures with `just gen-contract-fixtures` from the repo
// root; never hand-edit the files in ../../fixtures/. When the Rust shape
// changes, update the mirror below to match — that's the point where a real
// shape change gets caught by `tsc`.

import { describe, expect, it } from 'vitest';
import advertisementJson from '../../fixtures/advertisement.json';
import attestationsJson from '../../fixtures/attestations.json';
import backupManifestJson from '../../fixtures/backup_manifest.json';
import fundsJson from '../../fixtures/funds.json';
import healthJson from '../../fixtures/health.json';
import mgAdminErrorJson from '../../fixtures/mg_admin_error.json';
import mgAdminErrorKindsJson from '../../fixtures/mg_admin_error_kinds.json';
import adminRequestsJson from '../../fixtures/mg_admin_requests.json';
import mgCollectGuardianFeesJson from '../../fixtures/mg_collect_guardian_fees.json';
import mgCollectGuardianFeesIncompleteJson from '../../fixtures/mg_collect_guardian_fees_incomplete.json';
import mgCollectGuardianFeesIncompleteIdleJson from '../../fixtures/mg_collect_guardian_fees_incomplete_idle.json';
import mgCollectGuardianFeesIncompleteRefreshJson from '../../fixtures/mg_collect_guardian_fees_incomplete_refresh.json';
import mgDecommissionSeatJson from '../../fixtures/mg_decommission_seat.json';
import mgGuardianFeesJson from '../../fixtures/mg_guardian_fees.json';
import mgGuardianFeesPolicyErrorJson from '../../fixtures/mg_guardian_fees_policy_error.json';
import mgHolderAuthorizationRefreshJson from '../../fixtures/mg_holder_authorization_refresh.json';
import mgMarkSupportReadJson from '../../fixtures/mg_mark_support_read.json';
import mgMnemonicJson from '../../fixtures/mg_mnemonic.json';
import mgOnboardAsNewJson from '../../fixtures/mg_onboard_as_new.json';
import mgOnboardAsNewAlreadyJson from '../../fixtures/mg_onboard_as_new_already.json';
import mgOnboardFromBackupJson from '../../fixtures/mg_onboard_from_backup.json';
import mgOnboardingJson from '../../fixtures/mg_onboarding.json';
import mgPaymentFederationsJson from '../../fixtures/mg_payment_federations.json';
import mgPayoutDestinationJson from '../../fixtures/mg_payout_destination.json';
import mgPayoutJobJson from '../../fixtures/mg_payout_job.json';
import mgPayoutJobCappedJson from '../../fixtures/mg_payout_job_capped.json';
import mgPayoutJobStatusJson from '../../fixtures/mg_payout_job_status.json';
import mgPlansJson from '../../fixtures/mg_plans.json';
import mgReenrollTelemetryJson from '../../fixtures/mg_reenroll_telemetry.json';
import mgSeatGuardianFeesJson from '../../fixtures/mg_seat_guardian_fees.json';
import mgSeatReadinessJson from '../../fixtures/mg_seat_readiness.json';
import mgSeatReportsJson from '../../fixtures/mg_seat_reports.json';
import mgSeatStatusJson from '../../fixtures/mg_seat_status.json';
import mgSeatsJson from '../../fixtures/mg_seats.json';
import mgSendSupportMessageJson from '../../fixtures/mg_send_support_message.json';
import mgSupportChatJson from '../../fixtures/mg_support_chat.json';
import pagingJson from '../../fixtures/paging.json';
import type {
  AdminError,
  AdminErrorKind,
  AdminRequest,
  AdminRequestName,
  AttestationListResponse,
  BackupManifest,
  CollectGuardianFeesResponse,
  DecommissionSeatResponse,
  GetAdvertisementStateResponse,
  GetFundsResponse,
  GetHealthResponse,
  GuardianFeesResponse,
  ListPaymentFederationsResponse,
  ListSeatsResponse,
  ListWalletOperationsResponse,
  MarkSupportReadResponse,
  OnboardAsNewResponse,
  OnboardFromBackupResponse,
  OnboardingResponse,
  PayoutDestinationResponse,
  PayoutJob,
  PayoutJobStatusResponse,
  ReenrollTelemetryResponse,
  RefreshHolderAuthorizationsResponse,
  SeatGuardianFee,
  SeatReport,
  SeatStatusResponse,
  SendSupportMessageResponse,
  ShowMnemonicResponse,
  ShowPlansResponse,
  ShowSeatReadinessResponse,
  SupportChatResponse,
  SupportMessage
} from '../index';

// --- compile-time mirrors: real object literals, so literal/enum types stay
// intact under `satisfies` (unlike the widened JSON-module imports above). ---

const healthMirror = {
  overall_status: 'healthy',
  mode: 'normal',
  components: [
    { component: 'daemon', status: 'healthy', detail: null, observed_at: 1721476800 },
    { component: 'wallet', status: 'healthy', detail: null, observed_at: 1721476800 },
    { component: 'gateway', status: 'healthy', detail: null, observed_at: 1721476800 },
    { component: 'chain_observer', status: 'healthy', detail: null, observed_at: 1721476800 }
  ],
  observed_at: 1721476800
} satisfies GetHealthResponse;

const fundsMirror = {
  balance: {
    spendable: 4_200_000,
    pending_incoming: 150_000,
    pending_outgoing: 50_000,
    in_flight_allocations: 800_000,
    fee_reserve: 150_000,
    available_balance: 3_250_000
  },
  replenishment: 'ok',
  gateway: {
    gateway_id: 'gw-signet-01',
    gateway_name: 'Mock Signet Gateway',
    status: 'available',
    available_amount: 3_000_000,
    observed_at: 1721476800
  },
  stability_pool: {
    status: 'available',
    available_amount: 250_000,
    observed_at: 1721476800
  },
  effective_liquidity: [
    { source_type: 'gateway', gateway_id: 'gw-signet-01', amount: 3_000_000 },
    { source_type: 'stability_pool', gateway_id: null, amount: 250_000 }
  ]
} satisfies GetFundsResponse;

const advertisementMirror = {
  advertisement: {
    payload: {
      version: 1,
      provider_pubkey: '02aa00000000000000000000000000000000000000000000000000000000000000',
      issued_at: 1784505600,
      expires_at: 1784509200,
      supported_sources: ['gateway', 'stability_pool'],
      holder_authorizations: [],
      policy: {
        accepted_attester_policies: [
          {
            attester_pubkey: '02aa00000000000000000000000000000000000000000000000000000000000000',
            verification_requirement: 'all_trusted'
          }
        ],
        supported_networks: ['signet']
      },
      display: {
        name: 'Mock FLIP',
        website: 'https://flip.example',
        contact: 'ops@flip.example'
      },
      api_endpoints: ['https://flip.example/api'],
      api_versions: [1],
      relay_hints: ['wss://relay.signet.example']
    },
    proof: { signature: [1, 2, 3, 4] }
  },
  publication_status: 'published',
  last_published_at: 1784505600,
  expires_at: 1784509200,
  withdrawn_at: null,
  relay_states: [
    {
      relay_url: 'wss://relay.signet.example',
      status: 'published',
      last_error: null,
      last_seen_at: 1784505600
    }
  ],
  ready: true,
  readiness: null,
  unverified_holder_authorization_count: 0
} satisfies GetAdvertisementStateResponse;

// Only an issuer authority installs: a Holder authorization and its backing
// badge arrive together in the Holder's published event and are enrolled from
// a relay, so neither is uploadable and neither can appear here.
const attestationsMirror = {
  payloads: [
    {
      id: 'att-issuer-authority-01',
      kind: 'issuer_authority',
      issuer: '03bb00000000000000000000000000000000000000000000000000000000000000',
      subject: { issuer: '03bb00000000000000000000000000000000000000000000000000000000000000' },
      ingested_at: 1784538000,
      valid: true
    }
  ]
} satisfies AttestationListResponse;

const backupManifestMirror = {
  version: 3,
  created_at: 1721476800,
  state_groups: [
    'provider_identity',
    'attestations',
    'wallet_client_state',
    'database',
    'operation_history',
    'operator_config',
    'external_dependencies'
  ],
  recovery_point: {
    quiesced_at: 1721476790,
    stores: ['sqlite', 'data_directory']
  }
} satisfies BackupManifest;

const pagingMirror = {
  operations: {
    items: [
      {
        operation_id: 'wop-0003',
        operation_type: 'deposit',
        amount: 1_000_000,
        status: 'confirmed',
        federation_id: null,
        created_at: 1721476800,
        updated_at: 1721477100
      },
      {
        operation_id: 'wop-0002',
        operation_type: 'gateway_funding',
        amount: 500_000,
        status: 'completed',
        federation_id: 'fed-gw-01',
        created_at: 1721390400,
        updated_at: 1721390700
      }
    ],
    next_page: 'wop-0001'
  }
} satisfies ListWalletOperationsResponse;

// --- MG (crates/mg/core/src/admin.rs) ---
//
// Almost none of the MG admin surface is a serde-derived struct: admin.rs
// hand-encodes the JSON, so these mirrors are the only compile-time statement
// that the TypeScript vocabulary and the Rust encoder agree. Each fixture is
// produced by the daemon's own shaper (see
// crates/mg/core/tests/support/contract_fixtures.rs).

const SEAT_ID = '0707070707070707070707070707070707070707070707070707070707070707';
const DECOMMISSIONED_SEAT_ID = '0808080808080808080808080808080808080808080808080808080808080808';
const FA_ID = '79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798';
const HOLDER_PUBKEY = 'c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5';
const MNEMONIC =
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
// Fleet::guardian_fee_account serializes a stability-pool Account *into a
// string*, so the wire value is JSON nested in JSON.
const REMITTANCE_ACCOUNT =
  '{"acc_type":"BtcDepositor","pub_keys":["034f355bdcb7cc0af728ef3cceb9615d90684bb5b2ca5f859ab0f0b704075871aa"],"threshold":1}';
const RECIPIENTS = `{"version":1,"recipients":[{"account":${REMITTANCE_ACCOUNT},"weight":1}]}`;
const PLAN = { InfiniteBestEffort: { price_msats: 50_000_000 } };
const CREATED_AT_MS = 1_753_500_000_000;

// Keyed by AdminRequestName, so a variant declared in TypeScript but absent
// from the Rust inventory fails to compile, and one present in Rust but missing
// here fails the equality assertion below.
const adminRequestsMirror = {
  ShowPlans: 'ShowPlans',
  SetPrice: { SetPrice: { price_msats: 50_000_000 } },
  ShowCapacity: 'ShowCapacity',
  ShowSeatReadiness: 'ShowSeatReadiness',
  SetCapacity: { SetCapacity: { max_seats: 4 } },
  ConfigureInitialOffer: {
    ConfigureInitialOffer: { max_seats: 4, price_msats: 50_000_000 }
  },
  ListPaymentFederations: 'ListPaymentFederations',
  PayoutDestination: 'PayoutDestination',
  SetPayoutDestination: { SetPayoutDestination: { destination: 'operator@example.com' } },
  SweepPaymentFees: {
    SweepPaymentFees: {
      federation_id: 'fed1fixturepaymentfederation',
      request_id: 'fixture-payment-payout'
    }
  },
  PayoutStatus: { PayoutStatus: { request_id: 'fixture-payment-payout' } },
  AwaitPayout: { AwaitPayout: { request_id: 'fixture-payment-payout' } },
  ListSeats: 'ListSeats',
  SeatStatus: { SeatStatus: { seat_id: SEAT_ID } },
  DecommissionSeat: { DecommissionSeat: { seat_id: SEAT_ID } },
  ReenrollTelemetry: 'ReenrollTelemetry',
  SupportChat: 'SupportChat',
  SendSupportMessage: { SendSupportMessage: { body: 'Seat 2 stopped after the update.' } },
  MarkSupportRead: { MarkSupportRead: { ids: ['c'.repeat(64)] } },
  GuardianFees: { GuardianFees: { seat_id: SEAT_ID, limit: 20 } },
  CollectGuardianFees: { CollectGuardianFees: { seat_id: SEAT_ID } },
  SweepGuardianFees: {
    SweepGuardianFees: { seat_id: SEAT_ID, request_id: 'fixture-guardian-payout' }
  },
  Onboarding: 'Onboarding',
  RefreshHolderAuthorizations: 'RefreshHolderAuthorizations',
  ShowMnemonic: 'ShowMnemonic',
  OnboardAsNew: { OnboardAsNew: { if_needed: true } },
  OnboardFromBackup: {
    OnboardFromBackup: { mnemonic: MNEMONIC, acknowledge_original_host_is_gone: true }
  }
} satisfies Record<AdminRequestName, AdminRequest>;

// Every AdminErrorKind, in the order Rust declares them. The Rust side builds
// this fixture by walking a `match` over the enum, so a kind added there and
// not added here fails the equality assertion below — and a kind written here
// that Rust does not have fails to type-check.
const mgAdminErrorKindsMirror = [
  'unparsable_request',
  'not_onboarded',
  'already_onboarded',
  'invalid_mnemonic',
  'restore_not_acknowledged',
  'unreadable_backup_document',
  'seat_directory_exists',
  'missing_guardian_archive',
  'other'
] satisfies AdminErrorKind[];

const mgAdminErrorMirror = {
  kind: 'seat_directory_exists',
  message:
    'seat 0707070707070707070707070707070707070707070707070707070707070707 would be restored over an existing seat directory'
} satisfies AdminError;

const mgPlansMirror = { plans: [PLAN] } satisfies ShowPlansResponse;

const mgSeatReadinessMirror = {
  ready_for_new_seats: false,
  report: {
    checked_at_ms: 1_700_000_000_000,
    relay: 'pass',
    discovery: 'discovery_record_missing',
    bitcoin: 'bitcoin_syncing',
    seats: 'seats_not_serving',
    uptime: 'uptime_7d_low',
    uptime_comparison: {
      scores: { '1d': 887, '7d': 899, '30d': 935 },
      minimums: { '1d': 700, '7d': 900, '30d': 930 },
      wait_secs: { '1d': 0, '7d': 3000, '30d': 0 }
    }
  }
} satisfies ShowSeatReadinessResponse;

const mgPaymentFederationsMirror = {
  federations: [
    {
      accepted: true,
      federation_id: 'fed1fixtureacceptedfederation',
      receivable: true,
      wallet: {
        active_operation_count: 1,
        available_ecash_msat: 350,
        drain_state: 'pending_wallet_work',
        economically_sweepable_recipient_msat: 0,
        encumbered_outgoing_msat: null,
        outgoing: [
          {
            contract_amount_msat: 3980000,
            encumbered_msat: null,
            has_active_state_machines: true,
            operation_id: '17d55b3cb3e9cd25035f6b8cf296284d4445ba9ea8568ccf5ab198d4df27a5ce',
            rail: 'lnv1',
            recipient_amount_msat: 3960152,
            state: 'pending'
          }
        ],
        query_errors: []
      }
    },
    {
      accepted: true,
      federation_id: 'fed1fixtureunreadablefederation',
      receivable: false,
      wallet: {
        active_operation_count: 0,
        available_ecash_msat: null,
        drain_state: 'unknown',
        economically_sweepable_recipient_msat: null,
        encumbered_outgoing_msat: null,
        outgoing: null,
        query_errors: ['available_ecash', 'economically_sweepable', 'outgoing_operations']
      }
    },
    {
      accepted: false,
      federation_id: 'fed1fixtureleftoverfederation',
      receivable: false,
      wallet: {
        active_operation_count: 0,
        available_ecash_msat: 0,
        drain_state: 'drained',
        economically_sweepable_recipient_msat: 0,
        encumbered_outgoing_msat: 0,
        outgoing: [],
        query_errors: []
      }
    }
  ]
} satisfies ListPaymentFederationsResponse;

const mgPayoutDestinationMirror = {
  destination: 'operator@example.com'
} satisfies PayoutDestinationResponse;

const mgPayoutJobMirror = {
  created_at_ms: 1_753_600_001_000,
  destination: 'operator@example.com',
  operation: {
    amount_msat: 250_000,
    capped: null,
    committed_at_ms: 1_753_600_002_000,
    operation_id: '0f7c1b9a3e5d4c2b8a6f0e1d2c3b4a5960718293a4b5c6d7e8f90a1b2c3d4e5f'
  },
  request_id: 'fixture-payout-request',
  scope: { federation_id: 'fed1fixturepayment', kind: 'payment_federation' }
} satisfies PayoutJob;

const mgPayoutJobCappedMirror = {
  ...mgPayoutJobMirror,
  operation: {
    ...mgPayoutJobMirror.operation,
    capped: { maximum_msat: 250_000, remaining_msat: 12_345_000 }
  }
} satisfies PayoutJob;

const mgPayoutJobStatusMirror = {
  job: mgPayoutJobMirror,
  payout: {
    contract_amount_msat: 251_000,
    encumbered_msat: 0,
    has_active_state_machines: false,
    operation_id: '0f7c1b9a3e5d4c2b8a6f0e1d2c3b4a5960718293a4b5c6d7e8f90a1b2c3d4e5f',
    rail: 'lnv2',
    recipient_amount_msat: 250_000,
    state: 'succeeded'
  }
} satisfies PayoutJobStatusResponse;

const mgSeatsMirror = {
  seats: [
    {
      seat_id: SEAT_ID,
      fi_id: FA_ID,
      plan: PLAN,
      created_at_ms: CREATED_AT_MS,
      payment_claim: { state: 'not_paid' },
      decommissioned: false,
      completion_callback: { state: 'not_configured' },
      backup: null
    },
    {
      seat_id: SEAT_ID,
      fi_id: FA_ID,
      plan: PLAN,
      created_at_ms: CREATED_AT_MS,
      payment_claim: { state: 'pending' },
      decommissioned: false,
      completion_callback: {
        state: 'pending',
        attempts: 2,
        next_attempt_at_ms: 1_753_600_060_000,
        last_reason: 'network'
      },
      backup: { published_at_ms: 1_753_600_010_000, archive_confirmed: false }
    },
    {
      seat_id: SEAT_ID,
      fi_id: FA_ID,
      plan: PLAN,
      created_at_ms: CREATED_AT_MS,
      payment_claim: { state: 'success', at_ms: 1_753_600_000_000 },
      decommissioned: false,
      completion_callback: { state: 'delivered', attempts: 1, at_ms: 1_753_600_030_000 },
      backup: { published_at_ms: 1_753_600_020_000, archive_confirmed: true }
    },
    {
      seat_id: SEAT_ID,
      fi_id: FA_ID,
      plan: PLAN,
      created_at_ms: CREATED_AT_MS,
      payment_claim: { state: 'already_spent', at_ms: 1_753_599_000_000 },
      decommissioned: false,
      completion_callback: {
        state: 'operator_blocked',
        attempts: 3,
        reason: 'gateway_origin_missing'
      },
      backup: null
    },
    {
      seat_id: DECOMMISSIONED_SEAT_ID,
      fi_id: FA_ID,
      plan: PLAN,
      created_at_ms: CREATED_AT_MS,
      payment_claim: { state: 'not_paid' },
      decommissioned: true,
      completion_callback: {
        state: 'terminal',
        attempts: 5,
        at_ms: CREATED_AT_MS,
        reason: 'decommissioned'
      },
      backup: { published_at_ms: 1_753_600_040_000, archive_confirmed: true }
    }
  ],
  backup_scan: { completed_at_ms: 1_753_600_050_000, pending_seats: 1 }
} satisfies ListSeatsResponse;

// Every SeatReport shape admin.rs::report_json can emit.
const mgSeatReportsMirror = [
  { state: 'decommissioned', at_ms: CREATED_AT_MS },
  { state: 'active', health: 'healthy', phase: 'created' },
  { state: 'active', health: 'failed', phase: 'dkg_in_progress' },
  { state: 'active', health: 'healthy', phase: 'running', invite_code: 'fed11fixtureinvitecode' },
  {
    state: 'active',
    health: 'unavailable',
    phase: 'data_loss',
    invite_code: 'fed11fixturelostinvite'
  }
] satisfies SeatReport[];

const readGuardianFee = {
  remittance_account: REMITTANCE_ACCOUNT,
  share_matches_policy: true,
  send_ppm: 1_000,
  our_weight: 1,
  total_weight: 1
};

// The three distinct failures/successes: no derivable account, an account whose
// policy read failed (there is no federation before DKG), and a full read.
const mgSeatGuardianFeesMirror = [
  { error: 'guardian-fee collection is unavailable' },
  { remittance_account: REMITTANCE_ACCOUNT, policy_error: 'seat has no federation yet' },
  readGuardianFee
] satisfies SeatGuardianFee[];

const mgSeatStatusMirror = {
  seat_id: SEAT_ID,
  fi_id: FA_ID,
  plan: PLAN,
  created_at_ms: CREATED_AT_MS,
  payment_claim: { state: 'success', at_ms: 1_753_600_000_000 },
  decommissioned: false,
  completion_callback: { state: 'delivered', attempts: 1, at_ms: 1_753_600_030_000 },
  backup: { published_at_ms: 1_753_600_020_000, archive_confirmed: true },
  report: {
    state: 'active',
    health: 'healthy',
    phase: 'running',
    invite_code: 'fed11fixtureinvitecode'
  },
  guardian_fee: readGuardianFee
} satisfies SeatStatusResponse;

const mgDecommissionSeatMirror = {
  decommissioned: true,
  already_decommissioned: false
} satisfies DecommissionSeatResponse;

const mgReenrollTelemetryMirror = {
  telemetry_reenrollment: 'scheduled'
} satisfies ReenrollTelemetryResponse;

const supportOperatorMessage = {
  id: 'a'.repeat(64),
  author: 'operator',
  body: 'Seat 2 stopped after the update.',
  created_at: 1700000000,
  unread: false
} satisfies SupportMessage;

const mgSupportChatMirror = {
  available: true,
  messages: [
    supportOperatorMessage,
    {
      id: 'b'.repeat(64),
      author: 'fedi',
      body: 'Thanks. Does the seat log show a DKG error?',
      created_at: 1700000600,
      unread: true
    }
  ],
  unread: 1
} satisfies SupportChatResponse;

const mgSendSupportMessageMirror = {
  message: supportOperatorMessage
} satisfies SendSupportMessageResponse;

const mgMarkSupportReadMirror = { unread: 0 } satisfies MarkSupportReadResponse;

const mgGuardianFeesMirror = {
  seat_id: SEAT_ID,
  federation_id: '2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a',
  remittance_account: REMITTANCE_ACCOUNT,
  collectable_msat: 2_250_000,
  staged_msat: 1_500_000,
  locked_msat: 500_000,
  idle_msat: 250_000,
  wallet: {
    active_operation_count: 0,
    available_ecash_msat: 8_000_000,
    drain_state: 'sweepable',
    economically_sweepable_recipient_msat: 7_950_000,
    encumbered_outgoing_msat: 0,
    outgoing: [],
    query_errors: []
  },
  // Deliberately larger than the two remittances below add up to: the lifetime
  // total is not a sum of the window, and a fixture where the two agreed would
  // let a consumer that totals the window pass.
  lifetime_remitted_msat: 41_500_000,
  policy: {
    configured: true,
    send_ppm: 1_000,
    recipients: RECIPIENTS,
    share_matches_policy: true,
    our_weight: 1,
    total_weight: 1
  },
  remittances: [
    {
      amount_msat: 1_200_000,
      txid: 'a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90',
      remitted_at_unix: 1_753_600_000,
      total_msat: 4_800_000,
      breakdown: [
        { module: 'ln', direction: 'outgoing', amount_msat: 3_000_000 },
        { module: 'mint', direction: 'incoming', amount_msat: 1_800_000 }
      ]
    },
    // A sealed breakdown that does not open is still money we were paid.
    {
      amount_msat: 300_000,
      txid: 'b1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90',
      breakdown_error: 'sealed breakdown could not be opened'
    }
  ]
} satisfies GuardianFeesResponse;

const mgGuardianFeesPolicyErrorMirror = {
  ...mgGuardianFeesMirror,
  policy: { policy_error: 'malformed live guardian-fee metadata' }
} satisfies GuardianFeesResponse;

const mgCollectGuardianFeesMirror = {
  claimed_msat: 1_750_000,
  awaiting_cycle_msat: 500_000
} satisfies CollectGuardianFeesResponse;

const mgCollectGuardianFeesIncompleteMirror = {
  claimed_msat: 1_750_000,
  awaiting_cycle_msat: null,
  incomplete: {
    phase: 'unlock',
    operation_submitted: false,
    error: 'guardian-fee unlock could not be submitted; collection stopped'
  }
} satisfies CollectGuardianFeesResponse;

const mgCollectGuardianFeesIncompleteIdleMirror = {
  claimed_msat: 0,
  awaiting_cycle_msat: 500_000,
  incomplete: {
    phase: 'idle_claim',
    operation_submitted: true,
    error:
      'guardian-fee idle-balance claim was submitted but did not complete; refresh status before retrying'
  }
} satisfies CollectGuardianFeesResponse;

const mgCollectGuardianFeesIncompleteRefreshMirror = {
  claimed_msat: 1_750_000,
  awaiting_cycle_msat: null,
  incomplete: {
    phase: 'balance_refresh',
    operation_submitted: false,
    error:
      'guardian-fee operations completed but the updated balance could not be read; refresh status before retrying'
  }
} satisfies CollectGuardianFeesResponse;

const mgOnboardingMirror = {
  fman_name: 'understood-markhor',
  service_pubkey: '02a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f9',
  service_nostr_pubkey: FA_ID,
  nostr: {
    state: 'authorization_observed',
    authorizations: 1,
    holders: [HOLDER_PUBKEY],
    checked_at: 1_760_000_000
  },
  fman_version: { current: '0.1.0', latest: '0.2.0', update_required: true },
  stage: 'complete',
  runtime: 'ready'
} satisfies OnboardingResponse;

// Refresh answers the same onboarding view it just updated.
const mgHolderAuthorizationRefreshMirror =
  mgOnboardingMirror satisfies RefreshHolderAuthorizationsResponse;

const mgMnemonicMirror = { mnemonic: MNEMONIC } satisfies ShowMnemonicResponse;

const mgOnboardAsNewMirror = { onboarded: 'new', seats: 0 } satisfies OnboardAsNewResponse;

const mgOnboardAsNewAlreadyMirror = { onboarded: 'already' } satisfies OnboardAsNewResponse;

const mgOnboardFromBackupMirror = {
  onboarded: 'restored',
  seats: 2,
  formed: 1
} satisfies OnboardFromBackupResponse;

describe('committed fixtures match their type-checked mirrors', () => {
  it('should keep health.json equal to the typed mirror', () => {
    expect(healthJson).toEqual(healthMirror);
  });

  it('should keep funds.json equal to the typed mirror', () => {
    expect(fundsJson).toEqual(fundsMirror);
  });

  it('should keep advertisement.json equal to the typed mirror', () => {
    expect(advertisementJson).toEqual(advertisementMirror);
  });

  it('should keep attestations.json equal to the typed mirror', () => {
    expect(attestationsJson).toEqual(attestationsMirror);
  });

  it('should keep backup_manifest.json equal to the typed mirror', () => {
    expect(backupManifestJson).toEqual(backupManifestMirror);
  });

  it('should keep paging.json equal to the typed mirror', () => {
    expect(pagingJson).toEqual(pagingMirror);
  });
});

describe('committed MG fixtures match their type-checked mirrors', () => {
  it('should keep mg_admin_requests.json equal to the typed mirror', () => {
    expect(adminRequestsJson).toEqual(adminRequestsMirror);
  });

  it('should keep mg_admin_error_kinds.json equal to the typed mirror', () => {
    expect(mgAdminErrorKindsJson).toEqual(mgAdminErrorKindsMirror);
  });

  it('should keep mg_admin_error.json equal to the typed mirror', () => {
    expect(mgAdminErrorJson).toEqual(mgAdminErrorMirror);
  });

  it('should keep mg_plans.json equal to the typed mirror', () => {
    expect(mgPlansJson).toEqual(mgPlansMirror);
  });

  it('should keep mg_seat_readiness.json equal to the typed mirror', () => {
    expect(mgSeatReadinessJson).toEqual(mgSeatReadinessMirror);
  });

  it('should keep mg_payment_federations.json equal to the typed mirror', () => {
    expect(mgPaymentFederationsJson).toEqual(mgPaymentFederationsMirror);
  });

  it('should keep mg_payout_destination.json equal to the typed mirror', () => {
    expect(mgPayoutDestinationJson).toEqual(mgPayoutDestinationMirror);
  });

  it('should keep mg payout job fixtures equal to the typed mirrors', () => {
    expect(mgPayoutJobJson).toEqual(mgPayoutJobMirror);
    expect(mgPayoutJobCappedJson).toEqual(mgPayoutJobCappedMirror);
    expect(mgPayoutJobStatusJson).toEqual(mgPayoutJobStatusMirror);
  });

  it('should keep mg_seats.json equal to the typed mirror', () => {
    expect(mgSeatsJson).toEqual(mgSeatsMirror);
  });

  it('should keep mg_seat_reports.json equal to the typed mirror', () => {
    expect(mgSeatReportsJson).toEqual(mgSeatReportsMirror);
  });

  it('should keep mg_seat_guardian_fees.json equal to the typed mirror', () => {
    expect(mgSeatGuardianFeesJson).toEqual(mgSeatGuardianFeesMirror);
  });

  it('should keep mg_seat_status.json equal to the typed mirror', () => {
    expect(mgSeatStatusJson).toEqual(mgSeatStatusMirror);
  });

  it('should keep mg_decommission_seat.json equal to the typed mirror', () => {
    expect(mgDecommissionSeatJson).toEqual(mgDecommissionSeatMirror);
  });

  it('should keep mg_reenroll_telemetry.json equal to the typed mirror', () => {
    expect(mgReenrollTelemetryJson).toEqual(mgReenrollTelemetryMirror);
    expect(mgSupportChatJson).toEqual(mgSupportChatMirror);
    expect(mgSendSupportMessageJson).toEqual(mgSendSupportMessageMirror);
    expect(mgMarkSupportReadJson).toEqual(mgMarkSupportReadMirror);
  });

  it('should keep mg_guardian_fees.json equal to the typed mirror', () => {
    expect(mgGuardianFeesJson).toEqual(mgGuardianFeesMirror);
  });

  it('should keep malformed guardian-fee policy reporting equal to the typed mirror', () => {
    expect(mgGuardianFeesPolicyErrorJson).toEqual(mgGuardianFeesPolicyErrorMirror);
  });

  it('should keep mg_collect_guardian_fees.json equal to the typed mirror', () => {
    expect(mgCollectGuardianFeesJson).toEqual(mgCollectGuardianFeesMirror);
  });

  it('should keep the incomplete guardian-fee fixture equal to the typed mirror', () => {
    expect(mgCollectGuardianFeesIncompleteJson).toEqual(mgCollectGuardianFeesIncompleteMirror);
  });

  it('should pin idle and balance-refresh incomplete guardian-fee phases', () => {
    expect(mgCollectGuardianFeesIncompleteIdleJson).toEqual(
      mgCollectGuardianFeesIncompleteIdleMirror
    );
    expect(mgCollectGuardianFeesIncompleteRefreshJson).toEqual(
      mgCollectGuardianFeesIncompleteRefreshMirror
    );
  });

  it('should keep mg_onboarding.json equal to the typed mirror', () => {
    expect(mgOnboardingJson).toEqual(mgOnboardingMirror);
  });

  it('should keep mg_holder_authorization_refresh.json equal to the typed mirror', () => {
    expect(mgHolderAuthorizationRefreshJson).toEqual(mgHolderAuthorizationRefreshMirror);
  });

  it('should keep mg_mnemonic.json equal to the typed mirror', () => {
    expect(mgMnemonicJson).toEqual(mgMnemonicMirror);
  });

  it('should keep mg_onboard_as_new.json equal to the typed mirror', () => {
    expect(mgOnboardAsNewJson).toEqual(mgOnboardAsNewMirror);
  });

  it('should keep mg_onboard_as_new_already.json equal to the typed mirror', () => {
    expect(mgOnboardAsNewAlreadyJson).toEqual(mgOnboardAsNewAlreadyMirror);
  });

  it('should keep mg_onboard_from_backup.json equal to the typed mirror', () => {
    expect(mgOnboardFromBackupJson).toEqual(mgOnboardFromBackupMirror);
  });
});

describe('the MG request inventory covers the declared vocabulary', () => {
  // Together with `satisfies Record<CoveredRequestName, AdminRequest>` on the
  // mirror, this is the two-way check: `tsc` rejects a UI variant with no Rust
  // fixture, and this rejects a Rust variant the UI never declared.
  it('should carry one entry per AdminRequest variant the daemon declares', () => {
    expect(Object.keys(adminRequestsJson).sort()).toEqual(Object.keys(adminRequestsMirror).sort());
  });

  it('should key each entry by the externally-tagged name of its request', () => {
    for (const [name, request] of Object.entries(adminRequestsJson)) {
      const tag = typeof request === 'string' ? request : Object.keys(request)[0];
      expect(tag).toBe(name);
    }
  });
});

describe('fixture invariants called out in the remediation brief', () => {
  it('health.json timestamps should be JSON numbers, not strings', () => {
    expect(typeof healthJson.observed_at).toBe('number');
    for (const component of healthJson.components) {
      expect(typeof component.observed_at).toBe('number');
    }
  });

  it('funds.json observed_at timestamps should be JSON numbers', () => {
    expect(typeof fundsJson.gateway.observed_at).toBe('number');
    expect(typeof fundsJson.stability_pool.observed_at).toBe('number');
  });

  it('advertisement.json timestamps should be JSON numbers throughout the signed payload', () => {
    expect(typeof advertisementJson.last_published_at).toBe('number');
    expect(typeof advertisementJson.expires_at).toBe('number');
    expect(typeof advertisementJson.advertisement.payload.issued_at).toBe('number');
    expect(typeof advertisementJson.advertisement.payload.expires_at).toBe('number');
  });

  it('attestations.json ingested_at timestamps should be JSON numbers', () => {
    for (const payload of attestationsJson.payloads) {
      expect(typeof payload.ingested_at).toBe('number');
    }
  });

  it('backup_manifest.json should list exactly the 7 real backup state groups', () => {
    expect(backupManifestJson.state_groups).toHaveLength(7);
    expect(new Set(backupManifestJson.state_groups).size).toBe(7);
    expect(typeof backupManifestJson.created_at).toBe('number');
  });

  it('paging.json should carry a paginated list with numeric operation timestamps', () => {
    expect(pagingJson.operations.items.length).toBeGreaterThan(0);
    expect(typeof pagingJson.operations.next_page).toBe('string');
    for (const operation of pagingJson.operations.items) {
      expect(typeof operation.created_at).toBe('number');
      expect(typeof operation.updated_at).toBe('number');
    }
  });
});
