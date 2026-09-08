// UI vocabulary derived from crates/fman/core/src/admin.rs
// (+ crates/fman/core/src/seat.rs, crates/fman/core/src/guardian_fee.rs,
// crates/service-fleet-manager/src/types.rs).
//
// The request side is no longer written here at all: AdminRequest comes from
// ./generated/adminRequest.ts, which crates/fman/core emits from the Rust enum.
//
// The response side is still hand-maintained, but not unchecked: crates/fman/core
// generates a fixture per response shape into ../fixtures/fman_*.json, and
// __tests__/contractFixtures.test.ts mirrors each of them as a typed literal. A
// rename on either side fails `tsc` or the Rust freshness test rather than
// reaching a screen. Generating the responses too is blocked on admin.rs
// building all 23 of them with `json!` rather than as serde structs.
//
// AdminRequest, Plan, PaymentClaimStatus, SeatPhase, and SeatReport are externally
// tagged (no #[serde(tag = ...)] anywhere in the Rust): a unit variant serializes as
// a bare JSON string ("ShowPlans"), a struct variant as a single-key object
// ({ "SeatStatus": { "seat_id": "..." } }). PaymentClaimStatus/SeatPhase/SeatReport
// carry no serde derive at all in Rust — admin.rs hand-encodes their JSON, so the
// shapes below are the literal, load-bearing wire contract, not a derived guess.

import type { FederationId } from './funds';
import type { AdminRequest as GeneratedAdminRequest } from './generated/adminRequest';

// Federation invite code (serde(transparent) string).
type InviteCode = string;

export type SeatId = string;
export type FiId = string;

export type SeatHealth = 'healthy' | 'unavailable' | 'failed';

// service-fleet-manager's Plan enum. `SubscriptionBased` is post-MVP wire
// vocabulary the v1 daemon refuses to offer — kept here because ShowPlans's type
// is still Vec<Plan>. There is no free variant: free is a zero price
// (crates/fman/core/src/db.rs::plans maps the stored price to one plan, and no
// stored price to an empty list).
export type Plan =
  | { InfiniteBestEffort: { price_msats: number } }
  | {
      SubscriptionBased: {
        initial_price_msats: number;
        renewal_price_msats: number;
        period: string;
      };
    };

export type PaymentClaimStatus =
  | { state: 'not_paid' }
  | { state: 'pending' }
  | { state: 'success'; at_ms: number }
  | { state: 'already_spent'; at_ms: number };

export type CompletionCallbackReason =
  | 'gateway_origin_missing'
  | 'gateway_origin_mismatch'
  | 'http_client_unavailable'
  | 'network'
  | 'gateway_unavailable'
  | 'rate_limited'
  | 'hook_not_found'
  | 'hook_expired_or_revoked'
  | 'max_uses_exceeded'
  | 'policy_rejected'
  | 'decommissioned'
  | 'superseded';

export type CompletionCallbackStatus =
  | { state: 'not_configured' }
  | {
      state: 'pending';
      attempts: number;
      next_attempt_at_ms: number;
      last_reason: CompletionCallbackReason | null;
    }
  | {
      state: 'operator_blocked';
      attempts: number;
      reason: CompletionCallbackReason;
    }
  | { state: 'delivered'; attempts: number; at_ms: number }
  | {
      state: 'terminal';
      attempts: number;
      at_ms: number;
      reason: CompletionCallbackReason;
    };

export type SeatPhase =
  | { phase: 'created' }
  | { phase: 'dkg_in_progress' }
  | { phase: 'data_loss'; invite_code: InviteCode }
  | { phase: 'running'; invite_code: InviteCode };

export type SeatReport =
  | { state: 'decommissioned'; at_ms: number }
  | ({ state: 'active'; health: SeatHealth } & SeatPhase);

export interface SeatSummary {
  seat_id: SeatId;
  fi_id: FiId;
  plan: Plan;
  created_at_ms: number;
  payment_claim: PaymentClaimStatus;
  completion_callback: CompletionCallbackStatus;
  decommissioned: boolean;
  // Last relay-confirmed publication of the seat's recovery document; null
  // means the relay holds nothing current for this seat.
  backup: SeatBackupStatus | null;
}

export interface SeatBackupStatus {
  published_at_ms: number;
  // The published document names a guardian archive whose chunks are
  // confirmed on the relay. A formed seat showing false is backed up in name
  // only.
  archive_confirmed: boolean;
}

// admin.rs::seat_guardian_fee_json — reported with the seat so an operator does
// not need a second verb to notice being cut out of the fee split. Reading it is
// best-effort, and the two failures are distinct: `error` means no account could
// be derived at all, `policy_error` means the account exists but the federation
// metadata could not be read (there is no federation before DKG).
export type SeatGuardianFee =
  | { error: string }
  | { remittance_account: string; policy_error: string }
  | {
      remittance_account: string;
      // True when the federation's recipient list still gives this FMan the
      // weight the policy expects. Named for what admin.rs emits — the
      // `pays_us` this once declared exists nowhere in the daemon.
      share_matches_policy: boolean;
      send_ppm: number | null;
      our_weight: number | null;
      total_weight: number | null;
    };

export interface PaymentFederation {
  federation_id: FederationId;
  // Whether this federation is in the accepted common setup-payment set.
  // A false value is a wallet-only leftover of a removed member.
  accepted: boolean;
  receivable: boolean;
  wallet: WalletDrainStatus;
}

export type WalletDrainQuery =
  | 'available_ecash'
  | 'economically_sweepable'
  | 'outgoing_operations'
  | 'inconsistent_snapshot';

export type WalletDrainState = 'drained' | 'sweepable' | 'pending_wallet_work' | 'unknown';

export interface WalletOutgoingOperation {
  operation_id: string;
  rail: 'lnv1' | 'lnv2';
  state: 'pending' | 'succeeded' | 'failed_or_refunded' | 'unknown';
  recipient_amount_msat: number;
  contract_amount_msat: number;
  encumbered_msat: number | null;
  has_active_state_machines: boolean;
}

export interface WalletDrainStatus {
  available_ecash_msat: number | null;
  economically_sweepable_recipient_msat: number | null;
  encumbered_outgoing_msat: number | null;
  outgoing: WalletOutgoingOperation[] | null;
  active_operation_count: number;
  query_errors: WalletDrainQuery[];
  drain_state: WalletDrainState;
}

// --- guardian fees (crates/fman/core/src/guardian_fee.rs) ---

export interface FeePolicy {
  // Both keys present, so payers will charge and remit.
  configured: boolean;
  send_ppm: number | null;
  // The raw recipient value, kept so an operator can see what the federation
  // carries even when this FMan cannot make sense of it.
  recipients: string | null;
  // FeePolicy::share_matches_policy() in crates/fman/core/src/guardian_fee.rs.
  share_matches_policy: boolean;
  // Null when the recipient list does not provably name this FMan.
  our_weight: number | null;
  total_weight: number | null;
}

export interface RemittanceBreakdownItem {
  module: string;
  direction: string;
  amount_msat: number;
}

// A remittance whose sealed breakdown does not open is still money we were paid,
// so the amount is always present and the failure surfaces as `breakdown_error`.
export interface Remittance {
  amount_msat: number;
  txid: string;
  remitted_at_unix?: number;
  total_msat?: number;
  breakdown?: RemittanceBreakdownItem[];
  breakdown_error?: string;
}

// `authorizations` is a count, not a list: crates/fman/core/src/directory.rs
// declares it `usize`. There is no `disabled` state — the daemon always has a
// directory presence.
//
// Four states, because "we have not looked yet" and "we looked and found
// nothing" are different facts and the operator needs different sentences for
// them. `authorization_observed` outranks the other three: retained
// authorizations are durable and re-verified before reuse, so a failed or empty
// read never demotes a fleet that has one.
//
// `checked_at` is seconds since the epoch, and is absent exactly where no read
// has produced one — so a consumer can say "not checked yet" rather than
// reading a zero as a timestamp.
export type OnboardingNostrStatus =
  | { state: 'checking' }
  | { state: 'not_observed'; checked_at: number }
  | {
      state: 'authorization_observed';
      authorizations: number;
      holders: string[];
      // Null when the authorizations came from the retained store and no read
      // has succeeded since this daemon started.
      checked_at: number | null;
    }
  // The daemon could not read the relay, and has nothing retained to fall back
  // on. `error` is for the operator to read, not for the UI to match on.
  | { state: 'relay_error'; error: string };

// The running package version beside the one the authenticated setup-payment
// publication names as latest (admin.rs::AdminRequest::Onboarding). `latest` is
// null until a publication has been admitted, which is the ordinary state of a
// daemon that has not reached the relay yet — not an error. `update_required` is
// the daemon's own SemVer comparison over the two; a consumer displays it rather
// than re-deriving it, because string ordering is not SemVer ordering.
export interface FmanVersionReport {
  current: string;
  latest: string | null;
  update_required: boolean;
}

export interface OnboardingResponse {
  stage: 'holder_authorization' | 'initial_offer' | 'complete';
  runtime: 'starting' | 'ready';
  recommended_max_seats?: number;
  minimum_max_seats?: number;
  fman_name: string;
  service_pubkey: string;
  service_nostr_pubkey: string;
  nostr: OnboardingNostrStatus;
  fman_version: FmanVersionReport;
}

// --- requests ---
//
// The daemon's verbs are generated from the Rust enum, not transcribed beside
// it: ./generated/adminRequest.ts is written by
// crates/fman/core/src/bin/gen_fman_admin_request_ts.rs and kept honest by
// crates/fman/core/tests/admin_request_ts.rs. Add a verb in Rust and it appears
// here; reshape one and this file changes with it.
//
// There is no hand-written member: a verb the daemon does not declare cannot be
// spelled here, so a client that calls one fails to compile rather than 422ing
// against a running fleet.
export type AdminRequest = GeneratedAdminRequest;

// The externally-tagged name of each request: the bare string for a unit
// variant, the single object key for a struct variant. Derived rather than
// listed, so it cannot fall behind AdminRequest.
type RequestName<R> = R extends string ? R : keyof R;
export type AdminRequestName = RequestName<AdminRequest>;

// --- responses, one per AdminRequest variant ---
export interface ShowPlansResponse {
  plans: Plan[];
}
// SetPrice answers with the offer it just stored, in the same shape ShowPlans
// reads it back — so a write needs no follow-up read.
export type SetPriceResponse = ShowPlansResponse;
export interface CapacityResponse {
  max_seats: number;
  available_slots: number;
}
export type ShowCapacityResponse = CapacityResponse;
export type SetCapacityResponse = CapacityResponse;
export interface ConfigureInitialOfferResponse extends ShowPlansResponse {
  onboarding: 'complete';
  max_seats: number;
}
export interface ListPaymentFederationsResponse {
  federations: PaymentFederation[];
}
// PayoutDestination and SetPayoutDestination answer the same view, so a write
// needs no follow-up read. Null means no destination is configured.
export interface PayoutDestinationResponse {
  destination: string | null;
}
export type SetPayoutDestinationResponse = PayoutDestinationResponse;
export type PayoutScope =
  | { kind: 'payment_federation'; federation_id: FederationId }
  | {
      kind: 'guardian_fee';
      federation_id: FederationId;
      seat_id: SeatId;
      invite_code: string;
    };
export interface PayoutJobOperation {
  operation_id: string;
  amount_msat: number;
  committed_at_ms: number;
}
export interface PayoutJob {
  request_id: string;
  scope: PayoutScope;
  destination: string;
  operation: PayoutJobOperation | null;
  created_at_ms: number;
}
export interface PayoutJobStatusResponse {
  job: PayoutJob;
  payout: WalletOutgoingOperation | null;
}
export type SweepPaymentFeesResponse = PayoutJob;
export type SweepGuardianFeesResponse = PayoutJob;
export type PayoutStatusResponse = PayoutJobStatusResponse;
export type AwaitPayoutResponse = PayoutJobStatusResponse;
export interface ListSeatsResponse {
  seats: SeatSummary[];
  // The backup worker's last completed reconciliation pass; null before the
  // first scan finishes or when no relay is configured. A completed_at_ms
  // far older than the scan cadence means the worker is wedged.
  backup_scan: BackupScanStatus | null;
}
export interface BackupScanStatus {
  completed_at_ms: number;
  pending_seats: number;
}
export interface SeatStatusResponse extends SeatSummary {
  report: SeatReport;
  guardian_fee: SeatGuardianFee;
}
export interface DecommissionSeatResponse {
  decommissioned: true;
  already_decommissioned: boolean;
}
// The rotated capability is deliberately absent from the response.
export interface ReenrollTelemetryResponse {
  telemetry_reenrollment: 'scheduled';
}
export type RefreshHolderAuthorizationsResponse = OnboardingResponse;
export interface GuardianFeesResponse {
  seat_id: SeatId;
  federation_id: FederationId;
  remittance_account: string;
  // What a collection would take now; the staged/locked/idle split is where it sits.
  collectable_msat: number;
  staged_msat: number;
  locked_msat: number;
  idle_msat: number;
  wallet: WalletDrainStatus;
  // Everything payers have ever remitted to this seat, swept funds included.
  // The only figure here that spans time — every other amount is a current
  // balance, and `remittances` is a display window with a limit. Read this for
  // "earned, all time"; totalling `remittances` gives the newest `limit`
  // entries and nothing older.
  lifetime_remitted_msat: number;
  policy: FeePolicy;
  remittances: Remittance[];
}
export interface CompleteGuardianFeeCollection {
  // Exact decimal msat strings preserve the Rust u64 range through JSON.
  // Terminal success newly observed by this invocation.
  claimed_msat: string;
  // All collection successes durably receipted by Fleet Manager.
  recorded_claimed_msat: string;
  // Locked deposits leave only at the next cycle turnover, so a collection
  // reports what it could take rather than promising an empty account.
  awaiting_cycle_msat: string;
  incomplete?: never;
}
export interface IncompleteGuardianFeeCollection {
  // Exact decimal msat strings preserve the Rust u64 range through JSON.
  // Only value confirmed by a terminal operation success is counted.
  claimed_msat: string;
  // All collection successes durably receipted by Fleet Manager.
  recorded_claimed_msat: string;
  // A post-failure read may itself fail, in which case the current balance is unknown.
  awaiting_cycle_msat: string | null;
  incomplete: {
    phase: 'idle_claim' | 'unlock' | 'receipt' | 'balance_refresh';
    operation_submitted: boolean;
    error: string;
  };
}
export type CollectGuardianFeesResponse =
  | CompleteGuardianFeeCollection
  | IncompleteGuardianFeeCollection;
export interface ShowMnemonicResponse {
  mnemonic: string;
}
// Answered only by a daemon that has not been onboarded (crates/fman/core/src/
// onboarding.rs), except for `OnboardAsNew { if_needed: true }`, which a running
// fleet answers with "already".
export type OnboardAsNewResponse = { onboarded: 'new'; seats: number } | { onboarded: 'already' };
export interface OnboardFromBackupResponse {
  onboarded: 'restored';
  seats: number;
  formed: number;
}

// --- the verb table: every name to its declared payload and its declared answer ---
//
// Both halves of a verb are declared above — the request in ./generated/
// adminRequest.ts, the response beside it here — but nothing joined the two, so a
// stand-in for the daemon could take any payload and answer any shape. The mock
// did exactly that (`(payload: unknown) => unknown` plus eleven hand-casts), and
// a `GuardianFees` answer missing the required `lifetime_remitted_msat` passed
// `tsc`. These two lookups are the join, so a stand-in is checked against the
// same declarations a caller reads.

// A unit variant is a bare string and carries nothing; a struct variant carries
// its payload under its own name.
type UnitRequest = Extract<AdminRequest, string>;

/** The payload declared for one verb — `undefined` where the variant is a unit. */
export type AdminRequestPayload<N extends AdminRequestName> = N extends UnitRequest
  ? undefined
  : Extract<AdminRequest, Record<N, unknown>> extends Record<N, infer Payload>
    ? Payload
    : never;

/** The response declared for each verb. Kept exhaustive by
 *  `AdminResponsesCoverEveryVerb` below, so a verb added in Rust cannot arrive
 *  here without an answer type. */
export interface AdminResponseByName {
  ShowPlans: ShowPlansResponse;
  SetPrice: SetPriceResponse;
  ShowCapacity: ShowCapacityResponse;
  SetCapacity: SetCapacityResponse;
  ListPaymentFederations: ListPaymentFederationsResponse;
  PayoutDestination: PayoutDestinationResponse;
  SetPayoutDestination: SetPayoutDestinationResponse;
  SweepPaymentFees: SweepPaymentFeesResponse;
  PayoutStatus: PayoutStatusResponse;
  AwaitPayout: AwaitPayoutResponse;
  ListSeats: ListSeatsResponse;
  SeatStatus: SeatStatusResponse;
  DecommissionSeat: DecommissionSeatResponse;
  ReenrollTelemetry: ReenrollTelemetryResponse;
  GuardianFees: GuardianFeesResponse;
  CollectGuardianFees: CollectGuardianFeesResponse;
  SweepGuardianFees: SweepGuardianFeesResponse;
  Onboarding: OnboardingResponse;
  RefreshHolderAuthorizations: RefreshHolderAuthorizationsResponse;
  ConfigureInitialOffer: ConfigureInitialOfferResponse;
  ShowMnemonic: ShowMnemonicResponse;
  OnboardAsNew: OnboardAsNewResponse;
  OnboardFromBackup: OnboardFromBackupResponse;
}

type Assert<T extends true> = T;

// Tuple-wrapped so the two unions are compared whole rather than distributed:
// a name on either side that the other does not carry fails this line.
export type AdminResponsesCoverEveryVerb = Assert<
  [AdminRequestName] extends [keyof AdminResponseByName]
    ? [keyof AdminResponseByName] extends [AdminRequestName]
      ? true
      : false
    : false
>;

// --- transport envelope (crates/fman/core/src/admin.rs::answer_one and
// crates/fman/core/src/admin_http.rs both wrap the same Result<Value, AdminError>
// over their respective transports) ---

// Why the fleet manager refused, as a value rather than as English. Mirrors
// `AdminErrorKind` in crates/fman/core/src/admin.rs, and is checked against the
// committed `fman_admin_error_kinds` fixture — which Rust generates from an
// exhaustive walk over the enum, so a kind cannot reach the wire without
// failing this mirror first.
//
// `other` is every refusal with no distinct operator action. Branch on the
// named kinds; read `other` as "show the message, offer nothing specific".
export type AdminErrorKind =
  | 'unparsable_request'
  | 'not_onboarded'
  | 'already_onboarded'
  | 'invalid_mnemonic'
  | 'restore_not_acknowledged'
  | 'unreadable_backup_document'
  | 'seat_directory_exists'
  | 'missing_guardian_archive'
  | 'other';

// `message` is the operator-facing sentence and is the only thing the CLI
// prints. It is not a stable identifier: reword it freely, and branch on `kind`.
export interface AdminError {
  kind: AdminErrorKind;
  message: string;
}

export type AdminResult<T> = { Ok: T } | { Err: AdminError };
