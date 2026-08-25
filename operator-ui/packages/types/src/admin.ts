// Mirrors crates/service-liquidity-manager/src/admin.rs (+ provisional_types.rs,
// crates/domain/src/lib.rs, crates/services/src/lib.rs). Hand-maintained: when the
// Rust admin surface changes, update this file in the same change. Wire values are
// the serde snake_case strings.

// --- leaf newtype aliases (all serde(transparent)) ---
export type GatewayId = string;
export type GatewayName = string;
export type AttestationPayloadId = string;
export type AttestationPayload = number[]; // Vec<u8>
export type RpcEndpointId = string;
export type RpcEndpointAddress = string;
export type RpcDiscoveryHint = string;
export type RpcProtocolName = string;
export type RpcTransportName = string;
export type DurationSecs = number; // u64 seconds
export type Sats = number; // u64
export type SecretString = string; // write-only; never returned
export type Url = string;
export type Pubkey = string;
export type Timestamp = number; // Unix seconds (Rust: serde(transparent) u64)

// --- enums ---
export type BitcoinNetwork = 'bitcoin' | 'testnet' | 'signet' | 'regtest';

export type SetupStatus = 'not_configured' | 'pending_validation' | 'ready';

export type ValidationStatus = 'passed' | 'failed' | 'not_run';

export type CapacityMode = 'available_funds' | 'explicit_cap';

export type SourceType = 'gateway' | 'stability_pool';

export type VerificationRequirement = 'all_trusted' | 'consensus_majority_trusted';

export type AttestationKind = 'holder_authorization' | 'issuer_credential' | 'issuer_authority';

// RpcTransport: unit variants are bare strings; Other carries data as an object.
export type RpcTransport = 'iroh' | 'http_json' | 'json_rpc' | { other: RpcTransportName };

// --- ServiceError (crates/services/src/lib.rs) ---
export type ServiceErrorCode =
  | 'invalid_argument'
  | 'permission_denied'
  | 'not_found'
  | 'unavailable'
  | 'failed_precondition'
  | 'internal'
  | 'unknown';

export interface ServiceError {
  code: ServiceErrorCode;
  message: string;
}

// --- chain observer backend (internally tagged: { type: 'esplora' | 'bitcoind', ... }) ---
export type ChainObserverBackend =
  | { type: 'esplora'; url: Url }
  | { type: 'bitcoind'; url: Url; username?: string | null };

export interface ChainObserverConfig {
  backend: ChainObserverBackend;
}

export type ChainObserverBackendView =
  | { type: 'esplora'; url: Url }
  | { type: 'bitcoind'; url: Url; username?: string | null; has_password: boolean };

export interface ChainObserverConfigView {
  backend: ChainObserverBackendView;
}

// --- gateway ---
export interface GatewayConfig {
  gateway_id?: GatewayId | null;
  gateway_name: GatewayName;
  admin_url: string;
  identity_metadata: [string, string][];
}

// --- gateway identity ---
//
// gateway_id is frozen at first setup and decides which gateway an accepted
// allocation pays, so it is read from the gateway rather than typed: a typo
// would be permanent. FLIP never sends it back to the gateway — it is a local
// label on completion evidence — so it only has to be stable and distinct.
export interface ProbeGatewayRequest {
  admin_url: string;
}

export interface ProbeGatewayResponse {
  gateway_id: GatewayId;
  network: BitcoinNetwork;
  lightning_alias: string;
}

// --- named secrets ---
//
// Secrets are written by name, never inside a configuration. A config write
// states the whole configuration, so a secret carried inside one has an absent
// case that has to mean something — and the two below used to mean opposite
// things: a missing gateway credential failed the whole save, while a missing
// chain-observer password deleted the stored one.
export type ConfigSecret = 'gateway_admin_credential' | 'chain_observer_password';

export type SecretUpdate =
  | { action: 'set'; value: SecretString }
  // The only way to remove a secret. Refused for the gateway credential, which
  // the daemon authenticates every gateway call with.
  | { action: 'clear' };

export interface SetConfigSecretRequest {
  secret: ConfigSecret;
  update: SecretUpdate;
}

export interface SetConfigSecretResponse {
  secret: ConfigSecret;
  present: boolean;
}

export interface GatewayConfigView {
  gateway_id: GatewayId; // required on the view
  gateway_name: GatewayName;
  admin_url: string;
  has_admin_credential: boolean;
  identity_metadata: [string, string][];
}

// --- capacity / funding / replenishment ---
export interface CapacityConfig {
  mode: CapacityMode;
  explicit_cap?: Sats | null;
  supported_sources: SourceType[];
}

// Collapsed to one reserve + one depth (Rust a4b6219): fee_reserve is a single
// Sats value; confirmations a single u32 depth for provider-wallet operations.
export interface FundingPolicyConfig {
  fee_reserve: Sats;
  confirmations: number; // u32
  // Minimum provider fee rate for stability-pool `deposit_to_provide`, in parts
  // per billion. serde(default) → 0 when absent.
  stability_pool_min_fee_rate_ppb: number;
  // How long a wallet send may stay `in_doubt` without resolving evidence
  // before it is escalated to `manual_review_required`, in seconds.
  // serde(default) → 21600 when absent.
  in_doubt_review_after_secs: number;
}

export interface ReplenishmentConfig {
  warning_threshold: Sats;
  critical_threshold: Sats;
}

// --- rpc endpoint / advertisement ---
export interface RpcEndpointConfig {
  endpoint_id?: RpcEndpointId | null;
  transport: RpcTransport;
  address: RpcEndpointAddress;
  discovery_hints: RpcDiscoveryHint[];
  rpc_protocol_name: RpcProtocolName;
}

export interface AdvertisementConfig {
  republish_interval: DurationSecs;
  ready_advertisement_enabled: boolean;
}

// --- provider policy / display ---
export interface AcceptedAttesterPolicy {
  attester_pubkey: Pubkey;
  verification_requirement: VerificationRequirement;
}

export interface ProviderPolicy {
  accepted_attester_policies: AcceptedAttesterPolicy[];
  supported_networks: BitcoinNetwork[];
}

export interface ProviderDisplay {
  name?: string | null;
  website?: Url | null;
  contact?: string | null;
}

// --- attestations (view summary) ---
export interface AttestationSummary {
  holder_authorizations: number;
  issuer_credentials: number;
  issuer_authorities: number;
  valid: number;
  invalid: number;
}

// --- SetupConfig (write) and SetupConfigView (read) ---
export interface SetupConfig {
  network: BitcoinNetwork;
  gateway: GatewayConfig;
  chain_observer: ChainObserverConfig;
  relays: Url[];
  capacity: CapacityConfig;
  funding_policy: FundingPolicyConfig;
  replenishment: ReplenishmentConfig;
  advertised_endpoint: RpcEndpointConfig;
  advertisement: AdvertisementConfig;
  provider_display?: ProviderDisplay | null;
  policy: ProviderPolicy;
}

export interface SetupConfigView {
  network: BitcoinNetwork;
  gateway: GatewayConfigView;
  chain_observer: ChainObserverConfigView;
  relays: Url[];
  capacity: CapacityConfig;
  funding_policy: FundingPolicyConfig;
  replenishment: ReplenishmentConfig;
  advertised_endpoint: RpcEndpointConfig;
  advertisement: AdvertisementConfig;
  provider_display?: ProviderDisplay | null;
  policy: ProviderPolicy;
  attestation_summary: AttestationSummary;
}

// --- validation ---
export interface SetupValidationCheck {
  name: string;
  status: ValidationStatus;
  detail?: string | null;
}

export interface SetupValidationSummary {
  status: ValidationStatus;
  checks: SetupValidationCheck[];
}

// --- method request/response shapes (admin.rs) ---
export type GetSetupStateRequest = null; // unit struct → null
export interface GetSetupStateResponse {
  status: SetupStatus;
  config: SetupConfigView | null;
  missing_fields: string[];
  validation: SetupValidationSummary | null;
}

export interface ValidateSetupRequest {
  candidate_config?: SetupConfig | null; // absent/null = validate current
}
export interface ValidateSetupResponse {
  validation: SetupValidationSummary;
}

export interface ApplySetupConfigRequest {
  config: SetupConfig;
}
export interface ApplySetupConfigResponse {
  status: SetupStatus;
  validation: SetupValidationSummary;
}

export interface AttestationInstallRequest {
  payload: AttestationPayload;
}
export interface AttestationInstallResponse {
  id: AttestationPayloadId;
  kind: AttestationKind;
}

// AttestationSubject: externally-tagged enum (no #[serde(tag)] override in the
// Rust source) → one key per variant.
export type AttestationSubject = { provider: Pubkey } | { holder: Pubkey } | { issuer: Pubkey };

export type AttestationListRequest = null; // unit struct → null

export interface AttestationPayloadInfo {
  id: AttestationPayloadId;
  kind: AttestationKind;
  issuer?: Pubkey | null;
  subject: AttestationSubject;
  ingested_at: Timestamp;
  valid: boolean;
}

export interface AttestationListResponse {
  payloads: AttestationPayloadInfo[];
}

// AttestationSelector: externally-tagged enum → { id } or { issuer }.
export type AttestationSelector = { id: AttestationPayloadId } | { issuer: Pubkey };

export interface AttestationRemoveRequest {
  target: AttestationSelector;
}

export type AttestationRemoveResponse = Record<string, never>; // unit struct → {}

// --- provider config (get/update, provider-facing subset of SetupConfig) ---
export type GetProviderConfigRequest = null; // unit struct → null

export interface GetProviderConfigResponse {
  config: SetupConfigView;
}

// ProviderDisplayPatch: adjacently-tagged enum, #[serde(tag = "action", content = "value")].
export type ProviderDisplayPatch = { action: 'set'; value: ProviderDisplay } | { action: 'clear' };

export interface ProviderConfigPatch {
  policy?: ProviderPolicy | null;
  relays?: Url[] | null;
  capacity?: CapacityConfig | null;
  funding_policy?: FundingPolicyConfig | null;
  replenishment?: ReplenishmentConfig | null;
  advertised_endpoint?: RpcEndpointConfig | null;
  advertisement?: AdvertisementConfig | null;
  provider_display?: ProviderDisplayPatch | null;
}

export interface UpdateProviderConfigRequest {
  patch: ProviderConfigPatch;
}

export interface UpdateProviderConfigResponse {
  config: SetupConfigView;
  validation: SetupValidationSummary;
}

// --- backup / restore ---
export type BackupArchive = string; // opaque handle, serde(transparent)

export type BackupStateGroup =
  | 'provider_identity'
  | 'attestations'
  | 'wallet_client_state'
  | 'database'
  | 'operation_history'
  | 'operator_config'
  | 'external_dependencies';

// A durable store captured under one recovery point.
export type BackupStore = 'sqlite' | 'data_directory';

// The one instant every archived store was captured at.
//
// FLIP's durable state spans SQLite and the target-Fedimint client
// directories. `create_backup` holds every periodic worker pass still, copies
// both, and only then releases and compresses, so the correspondence is a
// property of the archive rather than of the daemon that wrote it.
export interface BackupRecoveryPoint {
  quiesced_at: Timestamp;
  // Copy order under the barrier, oldest first.
  stores: BackupStore[];
}

export interface BackupManifest {
  version: number; // ProtocolVersion, u16 on the wire
  created_at: Timestamp;
  state_groups: BackupStateGroup[];
  recovery_point: BackupRecoveryPoint;
}

export type CreateBackupRequest = null; // unit struct → null

export interface CreateBackupResponse {
  archive: BackupArchive;
  manifest: BackupManifest;
}

export interface InspectBackupRequest {
  archive: BackupArchive;
}

export interface InspectBackupResponse {
  manifest: BackupManifest;
}

export interface RestoreBackupRequest {
  archive: BackupArchive;
}

export interface RestoreBackupResponse {
  status: SetupStatus;
  validation: SetupValidationSummary;
  restored_state_groups: BackupStateGroup[];
}

export type AdvertisementPublicationStatus =
  | 'not_ready'
  | 'published'
  | 'stale'
  | 'withdrawn'
  | 'failed';

// Advertisement state / relay / mutation shapes live in ./advertisement.

// --- Holder authorization state (admin.rs) ---
// `get_holder_authorization_state` and `refresh_holder_authorizations`. The
// refresh runs on operator request rather than continuously, and a partial
// answer — some relays failed — is a success that names them, not an error.

export interface RelayFetchFailure {
  relay_url: Url;
  reason: string;
}

// Serde-tagged enum: `{ "status": "checking" }` or
// `{ "status": "authorization_observed", "authorizations": 2, ... }`.
export type HolderAuthorizationStatus =
  | { status: 'checking' }
  | { status: 'not_observed'; read_completed_at: Timestamp }
  | {
      status: 'authorization_observed';
      authorizations: number;
      holders: Pubkey[];
      newest_ingested_at: Timestamp;
    }
  | { status: 'relay_error'; reason: string; failed_at: Timestamp };

export type GetHolderAuthorizationStateRequest = Record<string, never>;

export interface GetHolderAuthorizationStateResponse {
  provider_pubkey?: Pubkey | null;
  status: HolderAuthorizationStatus;
}

// Named with the `Provider` prefix `admin.ts` already uses for FLIP-side
// concepts, because FMan's `fleet.ts` holds the bare
// `RefreshHolderAuthorizationsResponse` for a verb of the same name and a
// different shape — `{ holder_authorization_refresh: 'scheduled' }`. `index.ts`
// re-exports every module flat, so the two cannot both be bare. The wire verb is
// still `refresh_holder_authorizations`.
export type RefreshProviderHolderAuthorizationsRequest = Record<string, never>;

export interface RefreshProviderHolderAuthorizationsResponse {
  relays_answered: number;
  relays_failed: RelayFetchFailure[];
  candidates_seen: number;
  candidates_verified: number;
  status: HolderAuthorizationStatus;
}
