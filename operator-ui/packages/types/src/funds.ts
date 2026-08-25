// Mirrors crates/service-liquidity-manager/src/admin.rs (funds + wallet operations)
// and types.rs (paging). Hand-maintained: when the Rust admin surface changes,
// update this file in the same change. Wire values are serde snake_case.
//
// Federation-centric model (post `delete request_id` refactor): wallet operations
// and failures reference the federation, not a per-request funding target.

import type { BitcoinNetwork, GatewayId, GatewayName, Sats, SourceType, Timestamp } from './admin';
import type { ManualOperationStatus } from './allocations';
import type { ListResponse, PageRequest, TimeRange } from './paging';

// --- leaf newtype aliases (serde(transparent)) ---
export type WalletOperationId = string;
export type FederationId = string;
export type ItemId = string;

// --- enums ---
export type ReplenishmentStatus = 'ok' | 'warning' | 'critical';

export type InventoryStatus = 'available' | 'unavailable' | 'disabled' | 'unknown';

export type WalletOperationType =
  | 'deposit'
  | 'withdrawal'
  | 'gateway_funding'
  | 'stability_pool_funding';

export type WalletOperationStatus =
  | 'pending'
  | 'broadcast'
  | 'confirmed'
  | 'completed'
  | 'in_doubt'
  | 'manual_review_required'
  | 'failed'
  | 'cancelled';

// --- operator-visible failure ---
export interface AdminFailure {
  code: string;
  message: string;
  occurred_at: Timestamp;
  federation_id?: FederationId | null;
  item_id?: ItemId | null;
}

// --- funds / inventory ---
export interface WalletBalanceSummary {
  spendable: Sats;
  pending_incoming: Sats;
  pending_outgoing: Sats;
  in_flight_allocations: Sats;
  fee_reserve: Sats;
  available_balance: Sats;
}

export interface GatewayInventoryState {
  gateway_id: GatewayId;
  gateway_name: GatewayName;
  status: InventoryStatus;
  available_amount: Sats;
  observed_at?: Timestamp | null;
}

export interface StabilityPoolInventoryState {
  status: InventoryStatus;
  available_amount: Sats;
  observed_at?: Timestamp | null;
}

export interface EffectiveLiquidityItem {
  source_type: SourceType;
  gateway_id?: GatewayId | null;
  amount: Sats;
}

export type GetFundsRequest = null; // unit struct → null
export interface GetFundsResponse {
  balance: WalletBalanceSummary;
  replenishment: ReplenishmentStatus;
  gateway: GatewayInventoryState;
  stability_pool: StabilityPoolInventoryState;
  effective_liquidity: EffectiveLiquidityItem[];
}

// --- wallet operations ---
export interface WalletOperationSummary {
  operation_id: WalletOperationId;
  operation_type: WalletOperationType;
  amount: Sats;
  status: WalletOperationStatus;
  federation_id?: FederationId | null;
  created_at: Timestamp;
  updated_at: Timestamp;
}

export interface WalletOperation {
  operation_id: WalletOperationId;
  operation_type: WalletOperationType;
  amount: Sats;
  address?: string | null;
  txid?: string | null;
  tx_vout?: number | null; // u32
  status: WalletOperationStatus;
  confirmation_count?: number | null;
  federation_id?: FederationId | null;
  item_id?: ItemId | null;
  created_at: Timestamp;
  updated_at: Timestamp;
  failure?: AdminFailure | null;
}

export interface ListWalletOperationsRequest {
  page: PageRequest;
  status_filter?: WalletOperationStatus | null;
  time_range?: TimeRange | null;
}

export interface ListWalletOperationsResponse {
  operations: ListResponse<WalletOperationSummary>;
}

export interface GetWalletOperationRequest {
  operation_id: WalletOperationId;
}

// The full operation. The list shape above carries no destination, no chain
// evidence and no failure detail, so a screen that has to act on one operation
// — resolving a send held for manual review — reads it through here.
export interface GetWalletOperationResponse {
  operation: WalletOperation;
}

// --- manual review ---

// What the operator established about a send the daemon could not settle.
export type ManualReviewResolution = 'completed' | 'failed' | 'safe_to_retry';

export interface ResolveManualReviewRequest {
  operation_id: WalletOperationId;
  resolution: ManualReviewResolution;
  // Required for 'completed', and rejected for the other two, which assert no
  // send happened.
  txid?: string | null;
  reason?: string | null;
}

export interface ResolveManualReviewResponse {
  status: ManualOperationStatus;
  operation?: WalletOperation | null;
  detail?: string | null;
}

// --- deposit / withdrawal ---
export interface CreateDepositAddressRequest {
  label?: string | null;
}

export interface CreateDepositAddressResponse {
  address: string;
  network: BitcoinNetwork;
  operation_id?: WalletOperationId | null;
}

export interface RequestWithdrawalRequest {
  withdrawal_intent_id: string;
  address: string;
  amount: Sats;
  fee_rate_sat_per_vbyte?: number | null; // u64
}

export interface RequestWithdrawalResponse {
  operation: WalletOperation;
}

// --- complete_review_without_evidence (admin.rs) ---
// The deliberate route through when FLIP cannot obtain chain evidence for a
// reviewed send. Ruling P made `resolve_manual_review`'s `completed` arm
// require evidence and refuse every case where FLIP cannot get it; this verb
// completes on the operator's assertion instead and records in the audit log
// that no evidence existed. The point of splitting them is that an unverified
// completion can no longer arrive through the verb that looks verified.

export interface CompleteReviewWithoutEvidenceRequest {
  operation_id: WalletOperationId;
  txid: string;
  // Required. The audit row is the only record that this completion was
  // asserted rather than verified.
  reason: string;
}

export interface CompleteReviewWithoutEvidenceResponse {
  status: ManualOperationStatus;
  detail?: string | null;
}
