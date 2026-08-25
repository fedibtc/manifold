// Mirrors crates/service-liquidity-manager/src/admin.rs (allocation surface) and
// public.rs (AllocationStatus). Hand-maintained: when the Rust admin API changes,
// update this file in the same change. Wire values are serde snake_case; ids/amounts/
// timestamps serialize as bare strings/numbers.
//
// Federation-centric model (post `delete request_id` refactor): the federation IS
// the allocation identity. The list summary carries per-source `gateway_status` /
// `stability_pool_status`; the detail carries the public `AllocationStatus` struct
// with a per-item `item_statuses` array.

import type {
  AcceptedAttesterPolicy,
  GatewayId,
  GatewayName,
  Pubkey,
  Sats,
  Timestamp
} from './admin';
import type {
  AdminFailure,
  FederationId,
  ItemId,
  WalletOperation,
  WalletOperationId
} from './funds';
import type { ListResponse, PageRequest, TimeRange } from './paging';

// Sha256 digest serializes as a JSON array of 32 bytes (public.rs `[u8; 32]`), not hex.
type Sha256Digest = number[];

// --- item lifecycle status (public.rs `ItemAllocationStatus`) ---
export type ItemAllocationStatus =
  | 'pending'
  | 'running'
  | 'action_required'
  | 'completed'
  | 'failed'
  | 'cancelled';

// --- per-item failure (public.rs `LiquidityFailure`) ---
export type LiquidityFailureCode =
  | 'request_expired'
  | 'policy_mismatch'
  | 'insufficient_provider_funds'
  | 'gateway_attach_failed'
  | 'withdraw_failed'
  | 'stability_pool_failed'
  | 'internal_error';

export interface LiquidityFailure {
  code: LiquidityFailureCode;
  reason?: string | null;
}

// --- item target (public.rs `AllocationItemTarget`, externally tagged) ---
export type AllocationItemTarget =
  | { gateway: { item_id: ItemId; gateway_id: GatewayId; gateway_name: GatewayName; amount: Sats } }
  | { stability_pool: { item_id: ItemId; amount: Sats } };

// --- completion evidence (public.rs `CompletionEvidence`, externally tagged) ---
export interface GatewayCompletionEvidence {
  gateway_id: GatewayId;
  fulfilled_amount: Sats;
  observed_gateway_balance: Sats;
  observed_at: Timestamp;
  withdrawal_txid?: string | null;
  wallet_operation_id?: WalletOperationId | null;
}

export interface StabilityPoolCompletionEvidence {
  fulfilled_amount: Sats;
  observed_provided_amount: Sats;
  observed_at: Timestamp;
  peg_in_operation_id?: string | null;
  stability_pool_deposit_operation_id?: string | null;
}

export type CompletionEvidence =
  | { gateway: GatewayCompletionEvidence }
  | { stability_pool: StabilityPoolCompletionEvidence };

// --- per-item status (public.rs `AllocationItemStatus`) ---
export interface AllocationItemStatus {
  target: AllocationItemTarget;
  status: ItemAllocationStatus;
  fulfilled_amount?: Sats | null;
  completion_evidence?: CompletionEvidence | null;
  failure?: LiquidityFailure | null;
  updated_at: Timestamp;
}

// --- public allocation status struct (public.rs `AllocationStatus`) ---
export interface AllocationStatus {
  details_payload_hash: Sha256Digest;
  provider_pubkey: Pubkey;
  item_statuses: AllocationItemStatus[];
}

// --- allocation summary (list row) and detail ---
export interface AdminAllocationSummary {
  federation_id: FederationId;
  gateway_status?: ItemAllocationStatus | null;
  stability_pool_status?: ItemAllocationStatus | null;
  committed_amount: Sats;
  created_at: Timestamp;
  updated_at: Timestamp;
}

export interface AdminAllocationDetail {
  federation_id: FederationId;
  status: AllocationStatus;
  wallet_operations: WalletOperation[];
  failures: AdminFailure[];
}

// --- method request/response shapes (admin.rs) ---
export interface ListAllocationsRequest {
  page: PageRequest;
  time_range?: TimeRange | null;
}

export interface ListAllocationsResponse {
  allocations: ListResponse<AdminAllocationSummary>;
}

export interface GetAdminAllocationRequest {
  federation_id: FederationId;
}

export interface GetAdminAllocationResponse {
  allocation: AdminAllocationDetail;
}

// ManualOperationStatus (admin.rs): the 4 idempotency outcomes, not just accepted/rejected.
export type ManualOperationStatus = 'accepted' | 'rejected' | 'not_found' | 'already_applied';

export interface RetryFundingStepRequest {
  federation_id: FederationId;
  item_id?: ItemId | null;
  operation_id?: WalletOperationId | null;
}

export interface RetryFundingStepResponse {
  status: ManualOperationStatus;
  detail?: string | null;
}

export interface CancelAllocationRequest {
  federation_id: FederationId;
  reason?: string | null;
}

export interface CancelAllocationResponse {
  status: ManualOperationStatus;
  allocation_status?: AllocationStatus | null;
  detail?: string | null;
}

// --- Manual operations added 2026-08-20 (admin.rs) ---
// `packages/types` mirrors the Rust admin surface by hand, and these eight verbs
// had drifted out of it. `release_federation_allocation` drifted the same day it
// was added; the rest had never been mirrored. Nothing here fails when the
// mirror is incomplete, which is why it stayed incomplete.

export interface AbandonTargetClientValueRequest {
  federation_id: FederationId;
  // Required. This writes off FLIP's ability to manage funds it already sent.
  reason: string;
}

export interface AbandonTargetClientValueResponse {
  status: ManualOperationStatus;
  abandoned_amount?: Sats | null;
  detail?: string | null;
}

export interface BindTargetDepositRequest {
  federation_id: FederationId;
  operation_id: string;
  reason?: string | null;
}

export interface BindTargetDepositResponse {
  status: ManualOperationStatus;
  detail?: string | null;
}

export interface ReleaseFederationAllocationRequest {
  federation_id: FederationId;
  // Required. This hands a federation to whoever requests it next.
  reason: string;
}

export interface ReleaseFederationAllocationResponse {
  status: ManualOperationStatus;
  previous_requester?: Pubkey | null;
  detail?: string | null;
}

export interface TargetDepositOperationView {
  operation_id: string;
  amount: Sats;
  outcome?: string | null;
  failure_detail?: string | null;
  created_at: Timestamp;
}

export interface InspectTargetClientRequest {
  federation_id: FederationId;
}

export interface InspectTargetClientResponse {
  spendable_balance: Sats;
  observed_provided_amount: Sats;
  liquidity_stats_json: string;
  recorded_deposit_operation_id?: string | null;
  deposit_operations: TargetDepositOperationView[];
  scan_complete: boolean;
}

// --- Verification summary (admin.rs) ---
// `get_verification_summary`. What the trust pipeline decided about one
// federation's request, kept for the operator after the decision.

export type VerificationCheckStatus = 'passed' | 'failed' | 'not_run';

export interface VerificationCheck {
  name: string;
  status: VerificationCheckStatus;
  subject?: string | null;
  detail?: string | null;
}

export interface VerificationSummary {
  federation_id: FederationId;
  policy_result: VerificationCheckStatus;
  seat_checks: VerificationCheck[];
  credential_checks: VerificationCheck[];
  revocation_checks: VerificationCheck[];
  accepted_attester_policy?: AcceptedAttesterPolicy | null;
  failure_reason?: string | null;
}

export interface GetVerificationSummaryRequest {
  federation_id: FederationId;
}

export interface GetVerificationSummaryResponse {
  summary: VerificationSummary;
}
