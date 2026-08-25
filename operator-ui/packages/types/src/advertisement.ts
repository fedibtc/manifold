// Mirrors the advertisement surface of crates/service-liquidity-manager/src/admin.rs
// (+ public.rs LiquidityProviderAdvertisement, crates/domain Signed<T>). Wire values
// are serde snake_case strings. Reuses AdvertisementPublicationStatus from admin.ts.

import type {
  AdvertisementPublicationStatus,
  ProviderDisplay,
  ProviderPolicy,
  Pubkey,
  SetupValidationSummary,
  SourceType,
  Timestamp,
  Url
} from './admin';

// Spec version for a signed document (u16 on the wire).
export type ProtocolVersion = number;

// Signature over canonical payload bytes (Vec<u8>).
export type Signature = number[];

export interface PayloadProof {
  signature: Signature;
}

// Signed deterministic protocol payload document (crates/domain Signed<T>).
export interface Signed<T> {
  payload: T;
  proof: PayloadProof;
}

// Public discovery event payload (public.rs LiquidityProviderAdvertisement).
// Holder authorizations are kept opaque — the operator dashboard never inspects them.
export interface LiquidityProviderAdvertisement {
  version: ProtocolVersion;
  provider_pubkey: Pubkey;
  issued_at: Timestamp;
  expires_at: Timestamp;
  supported_sources: SourceType[];
  holder_authorizations: unknown[];
  policy: ProviderPolicy;
  display?: ProviderDisplay | null;
  api_endpoints: Url[];
  api_versions: ProtocolVersion[];
  relay_hints: Url[];
}

export type RelayStatus = 'connected' | 'disconnected' | 'published' | 'failed';

export interface RelayPublicationState {
  relay_url: Url;
  status: RelayStatus;
  last_error: string | null;
  last_seen_at: Timestamp | null;
}

export type GetAdvertisementStateRequest = null;

export interface GetAdvertisementStateResponse {
  advertisement: Signed<LiquidityProviderAdvertisement> | null;
  publication_status: AdvertisementPublicationStatus;
  last_published_at: Timestamp | null;
  expires_at: Timestamp | null;
  // When the operator last withdrew, if the advertisement is still withdrawn.
  // A withdrawal is durable: the publisher leaves the provider off the relays
  // while this is set, and only an explicit republish puts it back.
  withdrawn_at: Timestamp | null;
  relay_states: RelayPublicationState[];
  ready: boolean;
  readiness: SetupValidationSummary | null;
  // How many holder authorizations in `advertisement` no longer verify. The
  // payload is returned exactly as signed, so a non-zero count is how the Admin
  // API reports envelopes it would no longer stand behind without breaking the
  // signature it returns alongside them.
  unverified_holder_authorization_count: number;
}

export interface RepublishAdvertisementRequest {
  force: boolean;
}

export interface RepublishAdvertisementResponse {
  publication_status: AdvertisementPublicationStatus;
  relay_states: RelayPublicationState[];
}

export interface WithdrawAdvertisementRequest {
  reason: string | null;
}

export interface WithdrawAdvertisementResponse {
  publication_status: AdvertisementPublicationStatus;
  relay_states: RelayPublicationState[];
}

export type RefreshRelaysRequest = null;

export interface RefreshRelaysResponse {
  relay_states: RelayPublicationState[];
}
