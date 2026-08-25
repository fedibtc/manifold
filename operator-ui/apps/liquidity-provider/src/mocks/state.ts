import type {
  AdminAllocationDetail,
  AdminAllocationSummary,
  AdvertisementPublicationStatus,
  AttestationPayloadInfo,
  GetFundsResponse,
  GetHealthResponse,
  LiquidityProviderAdvertisement,
  RelayPublicationState,
  ServiceErrorCode,
  SetupConfig,
  SetupConfigView,
  SetupStatus,
  SetupValidationSummary,
  Signed,
  Timestamp,
  WalletOperationSummary
} from '@operator-ui/types';
import type { StoredSecrets } from '@/mocks/logic';
import { mockStore } from '@/mocks/store';

export interface MockState {
  setup: {
    status: SetupStatus;
    config: SetupConfigView | null; // persisted applied config (read shape)
    draft: SetupConfig | null; // last applied candidate while pending_validation
    missingFields: string[];
    validation: SetupValidationSummary | null;
    // Named secrets the mock daemon holds. Not part of the config: a config
    // write cannot set, keep or remove one, which is the whole point of the
    // separate verb.
    secrets: StoredSecrets;
  };
  advertisement: {
    publicationStatus: AdvertisementPublicationStatus;
    ready: boolean;
    view: Signed<LiquidityProviderAdvertisement> | null;
    relayStates: RelayPublicationState[];
    lastPublishedAt: Timestamp | null;
    expiresAt: Timestamp | null;
    // Set while the operator has withdrawn. The tick below leaves it alone, the
    // same way the daemon's publisher does: a withdrawal that the next
    // reconcile pass undoes is the defect this models the absence of.
    withdrawnAt: Timestamp | null;
  };
  funds: GetFundsResponse;
  health: GetHealthResponse;
  walletOperations: WalletOperationSummary[];
  allocations: {
    summaries: AdminAllocationSummary[];
    details: Record<string, AdminAllocationDetail>;
  };
  attestations?: AttestationPayloadInfo[];
  phase: 9 | 10 | 11; // daemon phase; gates deferred routes
  bootMode: 'normal' | 'restore';
  latencyMs: number;
  forcedErrors: Partial<Record<string, ServiceErrorCode | '503'>>; // per-method injection
}

export interface PatchInput {
  phase?: MockState['phase'];
  bootMode?: MockState['bootMode'];
  latencyMs?: number;
  path?: string;
  value?: unknown;
}

export const getState = (): MockState => mockStore.getWorld();

export const setState = (next: MockState): void => {
  Object.assign(mockStore.getWorld(), next);
  mockStore.persist();
  mockStore.notify();
};

const setByPath = (target: MockState, path: string, value: unknown): void => {
  const keys = path.split('.');
  const last = keys.pop();
  if (!last) return;
  let node: Record<string, unknown> = target as unknown as Record<string, unknown>;
  for (const key of keys) {
    const nextNode = node[key];
    if (typeof nextNode !== 'object' || nextNode === null) return;
    node = nextNode as Record<string, unknown>;
  }
  node[last] = value;
};

export const patchState = (patch: PatchInput): void => {
  const current = mockStore.getWorld();
  if (patch.phase !== undefined) current.phase = patch.phase;
  if (patch.bootMode !== undefined) current.bootMode = patch.bootMode;
  if (patch.latencyMs !== undefined) current.latencyMs = patch.latencyMs;
  if (patch.path !== undefined) setByPath(current, patch.path, patch.value);
  mockStore.persist();
  mockStore.notify();
};

/** Force a verb to fail with a `ServiceErrorCode` (or a bare `503`), or clear it
 *  with `null`. Shared by the dev panel and `window.__mockControl`, so both
 *  behave identically. */
export const setForcedError = (method: string, code: ServiceErrorCode | '503' | null): void => {
  const { forcedErrors } = getState();
  if (code === null) delete forcedErrors[method];
  else forcedErrors[method] = code;
  mockStore.persist();
  mockStore.notify();
};

export const resetState = (name?: string): void => {
  if (name === undefined) mockStore.reset();
  else mockStore.setScenario(name);
};

// Deterministic republish step used by /__control/tick.
export const tick = (): void => {
  const current = mockStore.getWorld();
  if (current.advertisement.withdrawnAt !== null) return;
  current.advertisement.publicationStatus = current.advertisement.ready ? 'published' : 'not_ready';
  mockStore.persist();
  mockStore.notify();
};
