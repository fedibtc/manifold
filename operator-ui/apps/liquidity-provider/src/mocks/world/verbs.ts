import {
  advertisementIssuedAt,
  backupManifest,
  publishedRelayStates,
  readyAdvertisement,
  withdrawnRelayStates
} from '@operator-ui/mock-fixtures';
import type {
  AdminAllocationDetail,
  ApplySetupConfigRequest,
  ApplySetupConfigResponse,
  AttestationInstallRequest,
  AttestationInstallResponse,
  AttestationListResponse,
  AttestationPayloadInfo,
  AttestationRemoveRequest,
  AttestationRemoveResponse,
  AttestationSelector,
  BackupManifest,
  CancelAllocationRequest,
  CancelAllocationResponse,
  CreateBackupResponse,
  CreateDepositAddressResponse,
  GetAdminAllocationRequest,
  GetAdminAllocationResponse,
  GetAdvertisementStateResponse,
  GetFundsResponse,
  GetHealthResponse,
  GetProviderConfigResponse,
  GetSetupStateResponse,
  GetWalletOperationRequest,
  GetWalletOperationResponse,
  InspectBackupRequest,
  InspectBackupResponse,
  ItemAllocationStatus,
  ListAllocationsRequest,
  ListAllocationsResponse,
  ListWalletOperationsResponse,
  ProbeGatewayRequest,
  ProbeGatewayResponse,
  RefreshRelaysResponse,
  RepublishAdvertisementResponse,
  RequestWithdrawalRequest,
  RequestWithdrawalResponse,
  ResolveManualReviewRequest,
  ResolveManualReviewResponse,
  RestoreBackupRequest,
  RestoreBackupResponse,
  RetryFundingStepRequest,
  RetryFundingStepResponse,
  ServiceErrorCode,
  SetConfigSecretRequest,
  SetConfigSecretResponse,
  SetupConfigView,
  SetupValidationSummary,
  UpdateProviderConfigRequest,
  UpdateProviderConfigResponse,
  ValidateSetupRequest,
  ValidateSetupResponse,
  WalletOperation,
  WithdrawAdvertisementResponse
} from '@operator-ui/types';
import { evaluateConfig, isMalformed, toView } from '@/mocks/logic';
import { getState } from '@/mocks/state';
import { withRestoreMarker } from '@/mocks/world/health';

export type Verb = (payload: unknown) => unknown;

export interface ServiceErrorLike {
  code: ServiceErrorCode;
  message: string;
}

export const isServiceErrorLike = (value: unknown): value is ServiceErrorLike =>
  typeof value === 'object' &&
  value !== null &&
  typeof (value as ServiceErrorLike).code === 'string' &&
  typeof (value as ServiceErrorLike).message === 'string';

const getSetupState: Verb = () => {
  const { setup } = getState();
  const body: GetSetupStateResponse = {
    status: setup.status,
    config: setup.config,
    missing_fields: setup.missingFields,
    validation: setup.validation
  };
  return body;
};

const validateSetup: Verb = (payload) => {
  const { setup } = getState();
  const request = (payload ?? {}) as ValidateSetupRequest;
  const candidate = request.candidate_config ?? setup.draft ?? null;
  const validation = candidate
    ? evaluateConfig(candidate)
    : (setup.validation ?? evaluateConfig(null));
  const body: ValidateSetupResponse = { validation };
  return body;
};

const applySetupConfig: Verb = (payload) => {
  const request = (payload ?? {}) as ApplySetupConfigRequest;
  const config = request.config;
  if (!config || isMalformed(config)) {
    throw {
      code: 'invalid_argument',
      message: 'config is structurally unusable'
    } satisfies ServiceErrorLike;
  }
  const state = getState();
  // The daemon's precondition, mirrored. A config write carries no credential,
  // so applying one before the credential is stored would leave a deployment
  // that cannot reach its gateway and no screen saying why.
  if (!state.setup.secrets.gateway_admin_credential) {
    throw {
      code: 'failed_precondition',
      message:
        'no gateway admin credential is stored: set it with set_config_secret before applying setup config'
    } satisfies ServiceErrorLike;
  }
  const validation = evaluateConfig(config);
  if (validation.status === 'passed') {
    state.setup = {
      status: 'ready',
      config: toView(config, state.setup.secrets),
      draft: null,
      missingFields: [],
      validation,
      secrets: state.setup.secrets
    };
    state.advertisement = {
      publicationStatus: 'published',
      ready: true,
      view: structuredClone(readyAdvertisement),
      relayStates: structuredClone(publishedRelayStates),
      lastPublishedAt: advertisementIssuedAt,
      expiresAt: readyAdvertisement.payload.expires_at,
      withdrawnAt: null
    };
    const body: ApplySetupConfigResponse = { status: 'ready', validation };
    return body;
  }
  state.setup = {
    status: 'pending_validation',
    config: state.setup.config,
    draft: config,
    missingFields: state.setup.missingFields,
    validation,
    secrets: state.setup.secrets
  };
  state.advertisement = {
    publicationStatus: 'not_ready',
    ready: false,
    view: null,
    relayStates: [],
    lastPublishedAt: null,
    expiresAt: null,
    withdrawnAt: null
  };
  const body: ApplySetupConfigResponse = { status: 'pending_validation', validation };
  return body;
};

const getProviderConfig: Verb = () => {
  const { setup } = getState();
  if (!setup.config) {
    throw {
      code: 'failed_precondition',
      message: 'setup has not completed'
    } satisfies ServiceErrorLike;
  }
  const body: GetProviderConfigResponse = { config: setup.config };
  return body;
};

// Merges the soft-field patch onto the live view. Hard fields
// (network/gateway/chain_observer) aren't part of ProviderConfigPatch, so
// they're never touched here.
const mergeProviderConfigPatch = (
  base: SetupConfigView,
  patch: UpdateProviderConfigRequest['patch']
): SetupConfigView => ({
  ...base,
  policy: patch.policy ?? base.policy,
  relays: patch.relays ?? base.relays,
  capacity: patch.capacity ?? base.capacity,
  funding_policy: patch.funding_policy ?? base.funding_policy,
  replenishment: patch.replenishment ?? base.replenishment,
  advertised_endpoint: patch.advertised_endpoint ?? base.advertised_endpoint,
  advertisement: patch.advertisement ?? base.advertisement,
  provider_display:
    patch.provider_display === undefined || patch.provider_display === null
      ? base.provider_display
      : patch.provider_display.action === 'set'
        ? patch.provider_display.value
        : null
});

const updateProviderConfig: Verb = (payload) => {
  const state = getState();
  if (!state.setup.config) {
    throw {
      code: 'failed_precondition',
      message: 'setup has not completed'
    } satisfies ServiceErrorLike;
  }
  const request = (payload ?? {}) as UpdateProviderConfigRequest;
  const nextView = mergeProviderConfigPatch(state.setup.config, request.patch ?? {});
  const validation: SetupValidationSummary = { status: 'passed', checks: [] };
  state.setup = { ...state.setup, config: nextView, validation };
  const body: UpdateProviderConfigResponse = { config: nextView, validation };
  return body;
};

// The identity the mock gateway reports for itself. A deterministic stand-in
// for a Lightning node public key — the wizard reads it rather than asking the
// operator to transcribe one.
const probeGateway: Verb = (payload) => {
  const { admin_url } = (payload ?? {}) as ProbeGatewayRequest;
  if (!admin_url?.trim()) {
    throw {
      code: 'invalid_argument',
      message: 'gateway.admin_url is required'
    } satisfies ServiceErrorLike;
  }
  // The daemon authenticates the probe with the stored credential, so a probe
  // before one is stored is a precondition failure, not a bad request.
  if (!getState().setup.secrets.gateway_admin_credential) {
    throw {
      code: 'failed_precondition',
      message: 'no gateway admin credential is stored'
    } satisfies ServiceErrorLike;
  }
  const body: ProbeGatewayResponse = {
    gateway_id: `02${'ab'.repeat(32)}`,
    network: 'signet',
    lightning_alias: 'mock-signet-gateway'
  };
  return body;
};

// Secrets are named and written on their own. A `set` with an empty value is
// refused rather than read as a removal — that misreading is the defect this
// verb exists to remove — and the gateway credential cannot be cleared at all,
// because the daemon authenticates every gateway call with it.
const setConfigSecret: Verb = (payload) => {
  const request = (payload ?? {}) as SetConfigSecretRequest;
  const { setup } = getState();
  if (request.update.action === 'set') {
    if (!request.update.value) {
      throw {
        code: 'invalid_argument',
        message: `${request.secret} must not be empty: use the clear operation to remove it`
      } satisfies ServiceErrorLike;
    }
    setup.secrets[request.secret] = true;
  } else {
    if (request.secret === 'gateway_admin_credential') {
      throw {
        code: 'invalid_argument',
        message:
          'the gateway admin credential cannot be cleared: the daemon authenticates every gateway call with it. Replace it instead'
      } satisfies ServiceErrorLike;
    }
    setup.secrets[request.secret] = false;
  }
  // The stored view's presence flags are projections of the secret store, so
  // they move with it rather than waiting for the next config write.
  if (setup.config) {
    setup.config.gateway.has_admin_credential = setup.secrets.gateway_admin_credential;
    if (setup.config.chain_observer.backend.type === 'bitcoind') {
      setup.config.chain_observer.backend.has_password = setup.secrets.chain_observer_password;
    }
  }
  const body: SetConfigSecretResponse = {
    secret: request.secret,
    present: setup.secrets[request.secret]
  };
  return body;
};

const getAdvertisementState: Verb = () => {
  const { setup, advertisement } = getState();
  const ready = advertisement.ready && setup.status === 'ready';
  const body: GetAdvertisementStateResponse = {
    advertisement: advertisement.view,
    publication_status: advertisement.publicationStatus,
    last_published_at: advertisement.lastPublishedAt,
    expires_at: advertisement.expiresAt,
    withdrawn_at: advertisement.withdrawnAt,
    relay_states: advertisement.relayStates,
    ready,
    readiness: setup.validation,
    // The mock world enrols only envelopes that verify, so this is always 0
    // here. A scenario that publishes an envelope and then revokes it would be
    // the way to exercise a non-zero count in the dashboard.
    unverified_holder_authorization_count: 0
  };
  return body;
};

// The dashboard sends force: true, which is the operator overriding their own
// withdrawal — the one way back onto the relays.
const republishAdvertisement: Verb = () => {
  const state = getState();
  const { advertisement } = state;
  if (advertisement.ready) {
    advertisement.withdrawnAt = null;
    advertisement.publicationStatus = 'published';
    advertisement.view = structuredClone(readyAdvertisement);
    advertisement.relayStates = structuredClone(publishedRelayStates);
    advertisement.lastPublishedAt = advertisementIssuedAt;
    advertisement.expiresAt = readyAdvertisement.payload.expires_at;
  } else {
    advertisement.publicationStatus = 'not_ready';
  }
  const body: RepublishAdvertisementResponse = {
    publication_status: advertisement.publicationStatus,
    relay_states: advertisement.relayStates
  };
  return body;
};

const refreshRelays: Verb = () => {
  const { advertisement } = getState();
  advertisement.relayStates = advertisement.relayStates.map((relay) =>
    relay.status === 'disconnected' ? { ...relay, status: 'connected' } : relay
  );
  const body: RefreshRelaysResponse = { relay_states: advertisement.relayStates };
  return body;
};

const withdrawAdvertisement: Verb = () => {
  const { advertisement } = getState();
  advertisement.publicationStatus = 'withdrawn';
  advertisement.view = null;
  advertisement.relayStates = structuredClone(withdrawnRelayStates);
  advertisement.expiresAt = null;
  // The durable part. Status alone is a report of the last publisher action and
  // the next tick overwrites it; this is what keeps the provider off the market.
  advertisement.withdrawnAt = advertisementIssuedAt;
  const body: WithdrawAdvertisementResponse = {
    publication_status: advertisement.publicationStatus,
    relay_states: advertisement.relayStates
  };
  return body;
};

const BECH32_CHARSET = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';

// Web Crypto's getRandomValues is a global in both Node and the browser, unlike
// node:crypto's randomBytes — this module now runs in both (Express + the MSW
// browser handlers), so it can't rely on a Node-only import.
const randomDepositAddress = (): string => {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  const payload = Array.from(bytes, (byte) => BECH32_CHARSET[byte % BECH32_CHARSET.length]).join(
    ''
  );
  return `tb1q${payload}`;
};

const getFunds: Verb = () => {
  const body: GetFundsResponse = getState().funds;
  return body;
};

const listWalletOperations: Verb = () => {
  const body: ListWalletOperationsResponse = {
    operations: { items: getState().walletOperations, next_page: null }
  };
  return body;
};

// The list carries a summary; this is the full row. The mock keeps only the
// summary in its world, so the detail is synthesised around it — deterministic,
// and shaped so the resolution screen has something to show for each field it
// renders.
const getWalletOperation: Verb = (payload) => {
  const { operation_id } = (payload ?? {}) as GetWalletOperationRequest;
  const summary = getState().walletOperations.find((row) => row.operation_id === operation_id);
  if (!summary) {
    throw {
      code: 'not_found',
      message: `wallet operation ${operation_id} not found`
    } satisfies ServiceErrorLike;
  }
  const body: GetWalletOperationResponse = {
    operation: {
      ...summary,
      address: 'tb1qmockwithdrawaldestination0000000000000000',
      // A send held for manual review is precisely one with no transaction the
      // daemon could confirm. That absence is the thing the operator is asked
      // to resolve, so the mock models it rather than inventing evidence.
      txid: summary.status === 'manual_review_required' ? null : 'a'.repeat(64),
      tx_vout: null,
      confirmation_count: null,
      item_id: null,
      failure:
        summary.status === 'manual_review_required'
          ? {
              code: 'gateway_unavailable',
              message: 'gateway did not answer the send',
              occurred_at: advertisementIssuedAt,
              federation_id: null,
              item_id: null
            }
          : null
    }
  };
  return body;
};

const resolveManualReview: Verb = (payload) => {
  const request = (payload ?? {}) as ResolveManualReviewRequest;
  const state = getState();
  const summary = state.walletOperations.find((row) => row.operation_id === request.operation_id);
  if (!summary) {
    const body: ResolveManualReviewResponse = {
      status: 'not_found',
      operation: null,
      detail: 'wallet operation not found'
    };
    return body;
  }
  if (summary.status !== 'manual_review_required') {
    const body: ResolveManualReviewResponse = {
      status: 'already_applied',
      operation: null,
      detail: `wallet operation is in state ${summary.status} and is not under manual review`
    };
    return body;
  }
  // The daemon's rule, mirrored: `completed` asserts a specific settlement and
  // is refused without the transaction that proves it, and the other two assert
  // no send happened so a transaction contradicts them.
  if (request.resolution === 'completed' && !request.txid) {
    throw {
      code: 'invalid_argument',
      message: 'txid is required to resolve as completed'
    } satisfies ServiceErrorLike;
  }
  if (request.resolution !== 'completed' && request.txid) {
    throw {
      code: 'invalid_argument',
      message: 'txid is only accepted with completed'
    } satisfies ServiceErrorLike;
  }
  summary.status = request.resolution === 'completed' ? 'completed' : 'failed';
  const body: ResolveManualReviewResponse = {
    status: 'accepted',
    operation: null,
    detail: `manual review resolved as ${request.resolution}`
  };
  return body;
};

const createDepositAddress: Verb = () => {
  const body: CreateDepositAddressResponse = {
    address: randomDepositAddress(),
    network: 'signet',
    operation_id: null
  };
  return body;
};

// Deterministic withdrawal acknowledgement echoing the requested address/amount.
const requestWithdrawal: Verb = (payload) => {
  const request = (payload ?? {}) as RequestWithdrawalRequest;
  const operation: WalletOperation = {
    operation_id: 'wop-withdraw-mock',
    operation_type: 'withdrawal',
    amount: request.amount ?? 0,
    address: request.address ?? null,
    txid: null,
    status: 'pending',
    confirmation_count: null,
    federation_id: null,
    item_id: null,
    created_at: 1721476800,
    updated_at: 1721476800,
    failure: null
  };
  const body: RequestWithdrawalResponse = { operation };
  return body;
};

const getHealth: Verb = () => {
  const { health, bootMode } = getState();
  const body: GetHealthResponse = withRestoreMarker(health, bootMode);
  return body;
};

// list_allocations — honors page.limit. Cursor paging is a stub (single page,
// next_page: null) — enough for the MVP screen.
const listAllocations: Verb = (payload) => {
  const request = (payload ?? {}) as ListAllocationsRequest;
  const { allocations } = getState();
  const limit = request.page?.limit ?? allocations.summaries.length;
  const items = allocations.summaries.slice(0, limit);
  const body: ListAllocationsResponse = {
    allocations: { items, next_page: null }
  };
  return body;
};

// get_allocation — detail lookup by federation_id; not_found when absent.
const getAllocation: Verb = (payload) => {
  const request = (payload ?? {}) as GetAdminAllocationRequest;
  const { allocations } = getState();
  const allocation = allocations.details[request.federation_id];
  if (!allocation) {
    throw { code: 'not_found', message: 'allocation not found' } satisfies ServiceErrorLike;
  }
  const body: GetAdminAllocationResponse = { allocation };
  return body;
};

const matchesRetryTarget = (
  operation: WalletOperation,
  request: RetryFundingStepRequest
): boolean => {
  if (request.operation_id) return operation.operation_id === request.operation_id;
  if (request.item_id) return operation.item_id === request.item_id;
  return true;
};

// retry_funding_step — not_found when the allocation is absent; flips a
// matching failed wallet-operation step (by item_id/operation_id, else any
// failed step) to pending, or reports not_found when none match.
const retryFundingStep: Verb = (payload) => {
  const request = (payload ?? {}) as RetryFundingStepRequest;
  const { allocations } = getState();
  const detail = allocations.details[request.federation_id];
  if (!detail) {
    throw { code: 'not_found', message: 'allocation not found' } satisfies ServiceErrorLike;
  }
  const failedOperations = detail.wallet_operations.filter(
    (operation) => operation.status === 'failed'
  );
  const target =
    request.operation_id || request.item_id
      ? failedOperations.find((operation) => matchesRetryTarget(operation, request))
      : failedOperations[0];
  if (!target) {
    const body: RetryFundingStepResponse = {
      status: 'not_found',
      detail: 'no matching failed step'
    };
    return body;
  }
  target.status = 'pending';
  const body: RetryFundingStepResponse = { status: 'accepted' };
  return body;
};

const TERMINAL_ITEM_STATUSES: ItemAllocationStatus[] = ['completed', 'cancelled'];

const isTerminal = (detail: AdminAllocationDetail): boolean =>
  detail.status.item_statuses.every((item) => TERMINAL_ITEM_STATUSES.includes(item.status));

const cancelIfActive = (status: ItemAllocationStatus | null): ItemAllocationStatus | null =>
  status && !TERMINAL_ITEM_STATUSES.includes(status) ? 'cancelled' : status;

// cancel_allocation — not_found when the allocation is absent; rejected when
// already terminal; otherwise cancels the active items on both the summary and
// the detail so the list and the timeline agree after invalidation.
const cancelAllocation: Verb = (payload) => {
  const request = (payload ?? {}) as CancelAllocationRequest;
  const { allocations } = getState();
  const detail = allocations.details[request.federation_id];
  const summary = allocations.summaries.find(
    (item) => item.federation_id === request.federation_id
  );
  if (!detail || !summary) {
    throw { code: 'not_found', message: 'allocation not found' } satisfies ServiceErrorLike;
  }
  if (isTerminal(detail)) {
    const body: CancelAllocationResponse = {
      status: 'rejected',
      detail: 'allocation already in a terminal state'
    };
    return body;
  }
  for (const item of detail.status.item_statuses) {
    item.status = cancelIfActive(item.status) ?? item.status;
  }
  summary.gateway_status = cancelIfActive(summary.gateway_status ?? null);
  summary.stability_pool_status = cancelIfActive(summary.stability_pool_status ?? null);
  const body: CancelAllocationResponse = {
    status: 'accepted',
    allocation_status: detail.status
  };
  return body;
};

let installCounter = 0;

const nextId = (): string => {
  installCounter += 1;
  return `att-installed-${installCounter}`;
};

// attestation_install — appends a holder_authorization payload marked valid.
// The mock does not parse the byte array; kind is always the default.
const attestationInstall: Verb = (payload) => {
  const request = (payload ?? {}) as AttestationInstallRequest;
  if (!Array.isArray(request.payload)) {
    throw {
      code: 'invalid_argument',
      message: 'payload must be a byte array'
    } satisfies ServiceErrorLike;
  }
  const state = getState();
  const id = nextId();
  const entry: AttestationPayloadInfo = {
    id,
    kind: 'holder_authorization',
    subject: { holder: '02mock'.padEnd(66, '0') },
    ingested_at: Math.floor(Date.now() / 1000),
    valid: true
  };
  state.attestations = [...(state.attestations ?? []), entry];
  const body: AttestationInstallResponse = { id, kind: 'holder_authorization' };
  return body;
};

// attestation_list — returns the current installed payloads.
const attestationList: Verb = () => {
  const body: AttestationListResponse = {
    payloads: getState().attestations ?? []
  };
  return body;
};

const matchesSelector = (entry: AttestationPayloadInfo, target: AttestationSelector): boolean => {
  if ('id' in target) return entry.id === target.id;
  return entry.issuer === target.issuer;
};

// attestation_remove — drops by id, or every payload whose issuer matches.
const attestationRemove: Verb = (payload) => {
  const request = (payload ?? {}) as AttestationRemoveRequest;
  if (!request.target) {
    throw { code: 'invalid_argument', message: 'target is required' } satisfies ServiceErrorLike;
  }
  const state = getState();
  state.attestations = (state.attestations ?? []).filter(
    (entry) => !matchesSelector(entry, request.target)
  );
  const body: AttestationRemoveResponse = {};
  return body;
};

// The mock never persists an archive server-side — create_backup is
// stateless, so the archive is a deterministic JSON envelope carrying its own
// manifest; inspect/restore just parse it back out. The manifest itself is the
// Rust-generated contract fixture, so the mock cannot drift from the daemon on
// the created_at codec or the state-group list.
interface MockArchivePayload {
  manifest: BackupManifest;
}

const parseArchive = (archive: string): MockArchivePayload | null => {
  try {
    const parsed = JSON.parse(archive) as MockArchivePayload;
    if (!parsed || typeof parsed !== 'object' || !parsed.manifest) return null;
    return parsed;
  } catch {
    return null;
  }
};

// create_backup — always succeeds with a fresh deterministic archive covering
// every BackupStateGroup.
const createBackup: Verb = () => {
  const archive = JSON.stringify({ manifest: backupManifest });
  const body: CreateBackupResponse = { archive, manifest: backupManifest };
  return body;
};

// inspect_backup — echoes the manifest embedded in a valid archive;
// invalid_argument for anything unparseable.
const inspectBackup: Verb = (payload) => {
  const request = (payload ?? {}) as InspectBackupRequest;
  const archivePayload = parseArchive(request.archive);
  if (!archivePayload) {
    throw {
      code: 'invalid_argument',
      message: 'archive is not a recognizable backup'
    } satisfies ServiceErrorLike;
  }
  const body: InspectBackupResponse = { manifest: archivePayload.manifest };
  return body;
};

// restore_backup — applies a valid archive's state groups; invalid_argument
// for anything unparseable.
const restoreBackup: Verb = (payload) => {
  const request = (payload ?? {}) as RestoreBackupRequest;
  const archivePayload = parseArchive(request.archive);
  if (!archivePayload) {
    throw {
      code: 'invalid_argument',
      message: 'archive is not a recognizable backup'
    } satisfies ServiceErrorLike;
  }
  const body: RestoreBackupResponse = {
    status: 'ready',
    validation: { status: 'passed', checks: [] },
    restored_state_groups: archivePayload.manifest.state_groups
  };
  return body;
};

export const verbs: Record<string, Verb> = {
  get_setup_state: getSetupState,
  validate_setup: validateSetup,
  apply_setup_config: applySetupConfig,
  set_config_secret: setConfigSecret,
  probe_gateway: probeGateway,
  get_provider_config: getProviderConfig,
  update_provider_config: updateProviderConfig,
  get_advertisement_state: getAdvertisementState,
  get_funds: getFunds,
  get_health: getHealth,
  list_wallet_operations: listWalletOperations,
  get_wallet_operation: getWalletOperation,
  resolve_manual_review: resolveManualReview,
  create_deposit_address: createDepositAddress,
  request_withdrawal: requestWithdrawal,
  republish_advertisement: republishAdvertisement,
  withdraw_advertisement: withdrawAdvertisement,
  refresh_relays: refreshRelays,
  list_allocations: listAllocations,
  get_allocation: getAllocation,
  retry_funding_step: retryFundingStep,
  cancel_allocation: cancelAllocation,
  attestation_install: attestationInstall,
  attestation_list: attestationList,
  attestation_remove: attestationRemove,
  create_backup: createBackup,
  inspect_backup: inspectBackup,
  restore_backup: restoreBackup
};

/** Verbs that change the world. The store persists only after these, so polling
 *  reads do not serialise the world on every tick. */
export const MUTATING_VERBS: ReadonlySet<string> = new Set([
  'apply_setup_config',
  'set_config_secret',
  'update_provider_config',
  'create_deposit_address',
  'request_withdrawal',
  'republish_advertisement',
  'withdraw_advertisement',
  'refresh_relays',
  'retry_funding_step',
  'resolve_manual_review',
  'cancel_allocation',
  'attestation_install',
  'attestation_remove',
  'restore_backup'
]);

export const adminMethods = Object.keys(verbs);

export const dispatch = (method: string, payload: unknown): unknown => {
  const verb = verbs[method];
  if (!verb)
    throw {
      code: 'unavailable',
      message: 'route not available in mock'
    } satisfies ServiceErrorLike;
  return verb(payload);
};
