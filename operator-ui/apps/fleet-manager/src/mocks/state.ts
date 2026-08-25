import type {
  FederationId,
  FeePolicy,
  OnboardingResponse,
  PaymentFederation,
  Remittance,
  SeatGuardianFee,
  SeatReport,
  SeatSummary
} from '@operator-ui/types';
import { mockStore } from '@/mocks/store';

/** The mutable fee ledger behind `GuardianFees`, `CollectGuardianFees` and
 *  `SweepGuardianFees` for one seat. `collectable_msat` is derived
 *  (staged + locked + idle), so it is not stored. */
export interface MockGuardianFees {
  federation_id: FederationId;
  remittance_account: string;
  staged_msat: number;
  locked_msat: number;
  idle_msat: number;
  collected_ecash_msat: number;
  /** Everything ever remitted to this seat, swept funds included. Stored rather
   *  than summed off `remittances`, because that list is a display window with a
   *  limit — the same reason the daemon carries it as its own scalar. */
  lifetime_remitted_msat: number;
  policy: FeePolicy;
  remittances: Remittance[];
}

export interface MockSeat extends SeatSummary {
  report: SeatReport;
  guardian_fee: SeatGuardianFee;
  /** Absent for a seat with no federation yet — `GuardianFees` errors on it,
   *  exactly as the daemon does before DKG. */
  fees?: MockGuardianFees;
}

/** What `OnboardFromBackup` reports having recovered. The daemon's own counts vary
 *  with the relay records behind the phrase; a fixed 2 / 1 could never produce the
 *  zero-seat screen. */
export type RestoreResultChoice = 'two-seats-one-formed' | 'two-seats-no-formed' | 'no-seats';

export const RESTORE_COUNTS: Record<RestoreResultChoice, { seats: number; formed: number }> = {
  'two-seats-one-formed': { seats: 2, formed: 1 },
  'two-seats-no-formed': { seats: 2, formed: 0 },
  'no-seats': { seats: 0, formed: 0 }
};

/** Where a recovery attempt loses its answer. Both failures happen at the HTTP
 *  boundary, never inside a verb: a lost response is not a daemon `{ Err }`, and
 *  the UI must be able to tell them apart. */
export type RestoreTransport = 'normal' | 'fail-before-dispatch' | 'fail-after-commit';

/** `expire-on-submit` applies to the NEXT OnboardFromBackup call only. Changing the
 *  control does not expire the session now, so the recovery form stays open until
 *  the tester submits it. */
export type RestoreSession = 'active' | 'expire-on-submit';

/** Fails the status check the unknown-result screen makes. Not a daemon `{ Err }`. */
export type OnboardingTransport = 'normal' | 'network-failure';

export interface MockState {
  /** Whether this host has been onboarded. A false value means only the
   *  onboarding verbs answer, mirroring crates/fman/core/src/onboarding.rs. */
  onboarded: boolean;
  seats: MockSeat[];
  paymentFederations: PaymentFederation[];
  /** The whole offer: millisatoshis per seat, or null for "not selling". */
  price: number | null;
  /** The one Lightning destination every revenue sweep leaves through. Null
   *  means the sweep verbs refuse rather than invent a payee. */
  payoutDestination: string | null;
  onboarding: OnboardingResponse;
  /** The durable seat-admission ceiling (`offer_state.max_seats`): written by
   *  `ConfigureInitialOffer` and `SetCapacity`, read by `ShowCapacity`. */
  maxSeats: number;
  /** How many `Onboarding` status reads still answer `runtime: starting` after
   *  the final stage is durable. The daemon reports `starting` until its fleet
   *  opens; the mock has no fleet to open, so completion arms a fixed number of
   *  `starting` reads and the one after them answers `ready` — deterministic
   *  for tests, and the gate observes the same starting→ready order the daemon
   *  produces. */
  fleetOpensAfterReads: number;
  /** Whether the relay currently holds a holder authorization for this fleet.
   *  `RefreshHolderAuthorizations` reads it the way the daemon's fetch reads
   *  the relay: present is retained and advances the wizard stage, absent
   *  leaves the wizard waiting at the authorization step. */
  relayAuthorization: 'present' | 'absent';
  authMode: 'password' | 'trusted_proxy';
  /** Whether a password login has succeeded. Lives in state (not a module
   *  variable) so it persists with the world and a refresh does not bounce the
   *  operator back to the login screen. */
  sessionActive: boolean;
  password: string;
  latencyMs: number;
  forcedErrors: Partial<Record<string, string>>;
  restoreResult: RestoreResultChoice;
  restoreTransport: RestoreTransport;
  restoreSession: RestoreSession;
  onboardingTransport: OnboardingTransport;
}

export interface PatchInput {
  latencyMs?: number;
  authMode?: MockState['authMode'];
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
  const current = getState();
  if (patch.latencyMs !== undefined) current.latencyMs = patch.latencyMs;
  if (patch.authMode !== undefined) current.authMode = patch.authMode;
  if (patch.path !== undefined) setByPath(current, patch.path, patch.value);
  mockStore.persist();
  mockStore.notify();
};

/** Force a verb to answer `{ Err: message }`, or clear it with `null`. Shared by
 *  the dev panel and `window.__mockControl`, so both behave identically. */
export const setForcedError = (method: string, message: string | null): void => {
  const { forcedErrors } = getState();
  if (message === null) delete forcedErrors[method];
  else forcedErrors[method] = message;
  mockStore.persist();
  mockStore.notify();
};

export const resetState = (name?: string): void => {
  if (name === undefined) mockStore.reset();
  else mockStore.setScenario(name);
};
