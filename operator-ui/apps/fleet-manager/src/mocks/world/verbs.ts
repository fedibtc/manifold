import type {
  AdminError,
  AdminErrorKind,
  AdminRequestName,
  AdminRequestPayload,
  AdminResponseByName,
  AdminResult,
  PayoutJob,
  Plan
} from '@operator-ui/types';
import { getState, type MockSeat, RESTORE_COUNTS } from '@/mocks/state';
import { walletStatus } from '@/mocks/wallet-status';
import { MOCK_HOLDER_PUBKEY } from '@/mocks/world/keys';

/** One verb, standing in for the daemon: it takes the payload the Rust enum
 *  declares for that name and answers the response type declared beside it. The
 *  mock is what every unit and e2e run reads, so it is held to the contract it
 *  is impersonating rather than to `unknown`. */
export type Verb<N extends AdminRequestName> = (
  payload: AdminRequestPayload<N>
) => AdminResponseByName[N];

/** Every verb the daemon declares, each at its own name. A verb added in Rust
 *  fails to compile here until it is answered. */
type VerbTable<N extends AdminRequestName> = { [K in N]: Verb<K> };

type OnboardingVerbName = 'OnboardAsNew' | 'OnboardFromBackup';

// A refusal that carries the daemon's discriminant. A verb that throws a plain
// Error still works and reports as `other`, which is what the daemon does with
// a failure it has no distinct action for.
class RefusalWithReason extends Error {
  readonly reason: AdminErrorKind;
  constructor(reason: AdminErrorKind, message: string) {
    super(message);
    this.reason = reason;
  }
}

const asAdminError = (error: unknown): AdminError => ({
  kind: error instanceof RefusalWithReason ? error.reason : 'other',
  message: error instanceof Error ? error.message : String(error)
});

const seatById = (seat_id: string): MockSeat => {
  const seat = getState().seats.find((candidate) => candidate.seat_id === seat_id);
  if (!seat) throw new Error('unknown seat');
  return seat;
};

const feeLedger = (seat: MockSeat) => {
  if (!seat.fees) throw new Error('seat has no federation yet');
  return seat.fees;
};

const listSeats: Verb<'ListSeats'> = () => ({
  seats: getState().seats.map(
    ({ report: _report, guardian_fee: _guardianFee, fees: _fees, ...summary }) => summary
  ),
  // `null` is what the daemon answers before the first backup scan finishes, or
  // with no relay configured (`Fleet::backup_scan`). This world runs no backup
  // worker, so it has no completed scan — reporting one would be a timestamp the
  // mock invented. Put it on mock state when a screen reads it.
  backup_scan: null
});

// SeatStatus is the only verb carrying health, phase and the fee summary — the
// list deliberately does not.
const seatStatus: Verb<'SeatStatus'> = ({ seat_id }) => {
  const { fees: _fees, ...seat } = seatById(seat_id);
  return seat;
};

const decommissionSeat: Verb<'DecommissionSeat'> = ({ seat_id }) => {
  const seat = seatById(seat_id);
  if (seat.decommissioned) {
    return { decommissioned: true, already_decommissioned: true };
  }
  seat.decommissioned = true;
  seat.report = { state: 'decommissioned', at_ms: Date.now() };
  return { decommissioned: true, already_decommissioned: false };
};

// One stored price is the whole offer: no price is an empty plan list, and a
// price of zero is a free — but still advertised — seat.
const plansForPrice = (price: number | null): Plan[] =>
  price === null ? [] : [{ InfiniteBestEffort: { price_msats: price } }];

const showPlans: Verb<'ShowPlans'> = () => ({ plans: plansForPrice(getState().price) });

const setPrice: Verb<'SetPrice'> = ({ price_msats }) => {
  if (price_msats !== null && (!Number.isInteger(price_msats) || price_msats < 0)) {
    throw new Error('price must be a whole number of millisatoshis');
  }
  getState().price = price_msats;
  return { plans: plansForPrice(price_msats) };
};

const listPaymentFederations: Verb<'ListPaymentFederations'> = () => ({
  federations: getState().paymentFederations
});

// `remittances` is a display window; `lifetime_remitted_msat` is the only figure
// here that spans time, so it is stored rather than summed off the window.
const guardianFees: Verb<'GuardianFees'> = ({ seat_id, limit }) => {
  const seat = seatById(seat_id);
  const ledger = feeLedger(seat);
  return {
    seat_id,
    federation_id: ledger.federation_id,
    remittance_account: ledger.remittance_account,
    collectable_msat: ledger.staged_msat + ledger.locked_msat + ledger.idle_msat,
    staged_msat: ledger.staged_msat,
    locked_msat: ledger.locked_msat,
    idle_msat: ledger.idle_msat,
    wallet: walletStatus(ledger.collected_ecash_msat),
    lifetime_remitted_msat: ledger.lifetime_remitted_msat,
    policy: ledger.policy,
    remittances: ledger.remittances.slice(0, limit ?? 20)
  };
};

// Locked deposits leave the pool only at the next cycle turnover, so a
// collection reports what it could take rather than emptying the account.
const collectGuardianFees: Verb<'CollectGuardianFees'> = ({ seat_id }) => {
  const ledger = feeLedger(seatById(seat_id));
  const claimed = ledger.staged_msat + ledger.idle_msat;
  ledger.staged_msat = 0;
  ledger.idle_msat = 0;
  ledger.collected_ecash_msat += claimed;
  return { claimed_msat: claimed, awaiting_cycle_msat: ledger.locked_msat };
};

// Every revenue sweep leaves through the one configured Lightning destination,
// so a fleet without one refuses rather than inventing a payee.
const payoutDestination: Verb<'PayoutDestination'> = () => ({
  destination: getState().payoutDestination
});

const setPayoutDestination: Verb<'SetPayoutDestination'> = ({ destination }) => {
  getState().payoutDestination = destination;
  return { destination };
};

const requirePayoutDestination = () => {
  if (!getState().payoutDestination) throw new Error('no payout destination configured');
};

const payoutJobs = new Map<string, PayoutJob>();

const sweepPaymentFees: Verb<'SweepPaymentFees'> = ({ federation_id, request_id }) => {
  const existing = payoutJobs.get(request_id);
  if (existing) {
    if (
      existing.scope.kind !== 'payment_federation' ||
      existing.scope.federation_id !== federation_id
    ) {
      throw new Error('payout request id is already bound to a different scope');
    }
    return existing;
  }
  requirePayoutDestination();
  const federation = getState().paymentFederations.find(
    (candidate) => candidate.federation_id === federation_id
  );
  if (!federation) throw new Error('unknown federation');
  const swept = federation.wallet.available_ecash_msat ?? 0;
  if (swept === 0) throw new Error('nothing to sweep');
  federation.wallet = walletStatus(0);
  const now = Date.now();
  const job: PayoutJob = {
    request_id,
    scope: { kind: 'payment_federation', federation_id },
    destination: getState().payoutDestination!,
    operation: {
      operation_id: `op_${federation_id.slice(4, 12)}_${swept}`,
      amount_msat: swept,
      committed_at_ms: now
    },
    created_at_ms: now
  };
  payoutJobs.set(request_id, job);
  return job;
};

const sweepGuardianFees: Verb<'SweepGuardianFees'> = ({ seat_id, request_id }) => {
  const existing = payoutJobs.get(request_id);
  if (existing) {
    if (existing.scope.kind !== 'guardian_fee' || existing.scope.seat_id !== seat_id) {
      throw new Error('payout request id is already bound to a different scope');
    }
    return existing;
  }
  requirePayoutDestination();
  const ledger = feeLedger(seatById(seat_id));
  const swept = ledger.collected_ecash_msat;
  if (swept === 0) throw new Error('nothing to sweep');
  ledger.collected_ecash_msat = 0;
  const now = Date.now();
  const job: PayoutJob = {
    request_id,
    scope: {
      kind: 'guardian_fee',
      federation_id: ledger.federation_id,
      seat_id,
      invite_code: `mock-invite-${ledger.federation_id}`
    },
    destination: getState().payoutDestination!,
    operation: {
      operation_id: `op_fees_${seat_id.slice(0, 8)}_${swept}`,
      amount_msat: swept,
      committed_at_ms: now
    },
    created_at_ms: now
  };
  payoutJobs.set(request_id, job);
  return job;
};

const payoutStatus: Verb<'PayoutStatus'> = ({ request_id }) => {
  const job = payoutJobs.get(request_id);
  if (!job) throw new Error('unknown payout request id');
  const operation = job.operation;
  return {
    job,
    payout:
      operation === null
        ? null
        : {
            operation_id: operation.operation_id,
            rail: 'lnv2',
            state: 'succeeded',
            recipient_amount_msat: operation.amount_msat,
            contract_amount_msat: operation.amount_msat,
            encumbered_msat: 0,
            has_active_state_machines: false
          }
  };
};

const awaitPayout: Verb<'AwaitPayout'> = payoutStatus;

/** The daemon derives this from available RAM (REQ-seat-capacity-default); the
 *  mock host is deterministic, so the recommendation is a constant. */
const MOCK_RECOMMENDED_MAX_SEATS = 8;

/** Seats the capacity ceiling must stay above. The daemon counts its seats
 *  table, which a restore repopulates; the mock world does not materialize
 *  restored seats, so the durable `minimum_max_seats` floor stands in for
 *  them beside the live list. */
const activeSeatCount = (): number => {
  const state = getState();
  const live = state.seats.filter((seat) => !seat.decommissioned).length;
  return Math.max(live, state.onboarding.minimum_max_seats ?? 0);
};

const onboarding: Verb<'Onboarding'> = () => {
  const state = getState();
  // Completion leaves `runtime: starting` until the "fleet opens": a counted
  // number of status reads, so tests observe starting before ready exactly as
  // an operator watching a real daemon does.
  if (state.onboarding.stage === 'complete' && state.onboarding.runtime === 'starting') {
    if (state.fleetOpensAfterReads > 0) {
      state.fleetOpensAfterReads -= 1;
    } else {
      state.onboarding = { ...state.onboarding, runtime: 'ready' };
    }
  }
  return state.onboarding;
};

// A fixed instant for the "last read" line the refresh stamps, for the same
// reason the scenarios pin theirs: a moving clock makes screen assertions flaky.
const REFRESH_READ_AT = 1_786_000_000;

/** One bounded relay read, as the daemon performs it: what the relay holds is
 *  retained, and retaining at least one authorization is what advances the
 *  wizard stage past `holder_authorization` — there is no skip. */
const refreshHolderAuthorizations: Verb<'RefreshHolderAuthorizations'> = () => {
  const state = getState();
  if (state.relayAuthorization === 'present') {
    state.onboarding = {
      ...state.onboarding,
      stage:
        state.onboarding.stage === 'holder_authorization'
          ? 'initial_offer'
          : state.onboarding.stage,
      nostr: {
        state: 'authorization_observed',
        authorizations: 1,
        holders: [MOCK_HOLDER_PUBKEY],
        checked_at: REFRESH_READ_AT
      }
    };
  } else {
    state.onboarding = {
      ...state.onboarding,
      // A relay that is down stays down across reads; the scenario says so.
      nostr:
        state.onboarding.nostr.state === 'relay_error'
          ? state.onboarding.nostr
          : { state: 'not_observed', checked_at: REFRESH_READ_AT }
    };
  }
  return state.onboarding;
};

const capacity = () => ({
  max_seats: getState().maxSeats,
  available_slots: Math.max(0, getState().maxSeats - activeSeatCount())
});

const showCapacity: Verb<'ShowCapacity'> = capacity;

// The durable ceiling never moves below seats that are still active,
// mirroring Db::set_max_seats and its error text.
const setCapacity: Verb<'SetCapacity'> = ({ max_seats }) => {
  const active = activeSeatCount();
  if (max_seats < active) {
    throw new Error(`cannot set max seats to ${max_seats}; ${active} seats are active`);
  }
  getState().maxSeats = max_seats;
  return capacity();
};

const configureInitialOffer: Verb<'ConfigureInitialOffer'> = ({ max_seats, price_msats }) => {
  const state = getState();
  // The same floor Db::configure_initial_offer holds: a restore's recovered
  // seats are already active, and the initial ceiling may not undercut them.
  const active = activeSeatCount();
  if (max_seats < active) {
    throw new Error(`cannot set max seats to ${max_seats}; ${active} seats are active`);
  }
  state.price = price_msats;
  state.maxSeats = max_seats;
  // The final stage is durable, but the daemon reports `starting` until its
  // fleet opens; arm one more `starting` status read before `ready`.
  state.onboarding = { ...state.onboarding, stage: 'complete', runtime: 'starting' };
  state.fleetOpensAfterReads = 1;
  return {
    onboarding: 'complete',
    max_seats,
    plans: price_msats === null ? [] : [{ InfiniteBestEffort: { price_msats } }]
  };
};

const reenrollTelemetry: Verb<'ReenrollTelemetry'> = () => ({
  telemetry_reenrollment: 'scheduled'
});

const showMnemonic: Verb<'ShowMnemonic'> = () => ({
  mnemonic:
    'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about'
});

// The two onboarding verbs are the only ones an un-onboarded host answers, and
// a running fleet refuses them — except `if_needed`, whose want is "ensure
// onboarded" and for which an already-onboarded host is the answer.
const onboardAsNew: Verb<'OnboardAsNew'> = ({ if_needed }) => {
  const state = getState();
  if (state.onboarded) {
    if (if_needed) return { onboarded: 'already' };
    throw new RefusalWithReason(
      'already_onboarded',
      'this Fleet Manager has already been onboarded; a host is set up once'
    );
  }
  state.onboarded = true;
  // The identity is only the first stage; the durable cursor now waits at the
  // authorization step, and the runtime is not `ready` until setup completes.
  // The wizard's capacity field seeds from the recommendation, so the status
  // the wizard reads must carry one, as the daemon's does.
  state.onboarding = {
    ...state.onboarding,
    stage: 'holder_authorization',
    runtime: 'starting',
    recommended_max_seats: MOCK_RECOMMENDED_MAX_SEATS,
    minimum_max_seats: 0
  };
  return { onboarded: 'new', seats: 0 };
};

const onboardFromBackup: Verb<'OnboardFromBackup'> = ({
  mnemonic,
  acknowledge_original_host_is_gone
}) => {
  if (getState().onboarded) {
    throw new RefusalWithReason(
      'already_onboarded',
      'this Fleet Manager has already been onboarded; a host is set up once'
    );
  }
  if (!acknowledge_original_host_is_gone) {
    throw new RefusalWithReason(
      'restore_not_acknowledged',
      'restore requires acknowledging that the original guardians are permanently offline'
    );
  }
  if (mnemonic.trim().split(/\s+/).length !== 12) {
    throw new RefusalWithReason('invalid_mnemonic', 'that is not a valid mnemonic phrase');
  }
  const state = getState();
  state.onboarded = true;
  // The daemon's counts come from the relay records behind the phrase, so they vary.
  // A fixed 2 / 1 could never produce the zero-seat screen a tester has to review.
  const { seats, formed } = RESTORE_COUNTS[state.restoreResult];
  // A restore recovers the fleet but not the offer, so it re-enters the wizard
  // at the authorization stage, exactly as install_restored_fleet records it.
  // Recovered seats floor the capacity the wizard may configure.
  state.onboarding = {
    ...state.onboarding,
    stage: 'holder_authorization',
    runtime: 'starting',
    recommended_max_seats: MOCK_RECOMMENDED_MAX_SEATS,
    minimum_max_seats: seats
  };
  return { onboarded: 'restored', seats, formed };
};

const onboardingHandlers: VerbTable<OnboardingVerbName> = {
  OnboardAsNew: onboardAsNew,
  OnboardFromBackup: onboardFromBackup
};

const fleetHandlers: VerbTable<Exclude<AdminRequestName, OnboardingVerbName>> = {
  ListSeats: listSeats,
  SeatStatus: seatStatus,
  DecommissionSeat: decommissionSeat,
  ReenrollTelemetry: reenrollTelemetry,
  ShowPlans: showPlans,
  SetPrice: setPrice,
  ShowCapacity: showCapacity,
  SetCapacity: setCapacity,
  ListPaymentFederations: listPaymentFederations,
  PayoutDestination: payoutDestination,
  SetPayoutDestination: setPayoutDestination,
  SweepPaymentFees: sweepPaymentFees,
  PayoutStatus: payoutStatus,
  AwaitPayout: awaitPayout,
  GuardianFees: guardianFees,
  CollectGuardianFees: collectGuardianFees,
  SweepGuardianFees: sweepGuardianFees,
  Onboarding: onboarding,
  RefreshHolderAuthorizations: refreshHolderAuthorizations,
  ConfigureInitialOffer: configureInitialOffer,
  ShowMnemonic: showMnemonic
};

export const verbs: VerbTable<AdminRequestName> = { ...fleetHandlers, ...onboardingHandlers };

/** Verbs that change the world. The store persists only after these, so polling
 *  reads (SeatStatus, Onboarding) do not serialise the world on every tick. */
const mutatingVerbNames: readonly AdminRequestName[] = [
  'DecommissionSeat',
  'SetPrice',
  'SetPayoutDestination',
  'SweepPaymentFees',
  'CollectGuardianFees',
  'SweepGuardianFees',
  'OnboardAsNew',
  'OnboardFromBackup',
  'RefreshHolderAuthorizations',
  'ConfigureInitialOffer',
  'SetCapacity'
];

// Exposed over `string` because the caller holds a name read off the wire.
export const MUTATING_VERBS: ReadonlySet<string> = new Set(mutatingVerbNames);

/**
 * Every dispatchable `AdminRequest` variant, derived from the verb map so the
 * dev control panel's error-injection list cannot drift from what is routed.
 *
 * The map itself is checked against the Rust request inventory
 * (`@operator-ui/types/fixtures/fman_admin_requests.json`) in
 * `src/mocks/__tests__/verb-catalogue.test.ts`, so the mock can neither miss a
 * verb the daemon added nor keep answering one it deleted.
 */
export const adminMethods = Object.keys(verbs);

// AdminRequest is externally tagged: a unit variant is a bare string ("ShowPlans"), a
// struct variant is a single-key object ({"SeatStatus":{"seat_id":"x"}}) — mirrors
// crates/fman/core/src/admin.rs's dispatch(), one POST /api/admin route, not a
// per-method URL path.
export const parseRequest = (body: unknown): { method: string; payload: unknown } => {
  if (typeof body === 'string') return { method: body, payload: undefined };
  const method = Object.keys(body as object)[0];
  return { method, payload: (body as Record<string, unknown>)[method] };
};

export const isOnboardingVerb = (method: string): boolean => method in onboardingHandlers;

const isAdminVerb = (method: string): method is AdminRequestName => method in verbs;

/** What each unfinished stage answers, mirroring `Onboarding::answer`'s
 *  per-stage match arms: the cursor decides which verbs exist, and everything
 *  else is refused rather than answered out of order. The daemon's initial-offer
 *  stage does not serve the refresh — the retained authorization is settled —
 *  and no stage before `complete` serves a fleet verb. */
const STAGE_VERBS: Record<'holder_authorization' | 'initial_offer', ReadonlySet<string>> = {
  holder_authorization: new Set([
    'Onboarding',
    'RefreshHolderAuthorizations',
    'ShowMnemonic',
    'OnboardAsNew',
    'OnboardFromBackup'
  ]),
  initial_offer: new Set([
    'Onboarding',
    'ShowMnemonic',
    'ConfigureInitialOffer',
    'OnboardAsNew',
    'OnboardFromBackup'
  ])
};

/** The refusal every stage gives a verb it does not serve
 *  (`fman_core::onboarding::NotOnboarded`). */
const notOnboardedRefusal = (): AdminResult<unknown> => ({
  Err: {
    kind: 'not_onboarded',
    message:
      'this Fleet Manager has not been onboarded yet: run `admin onboard new` or `admin onboard restore`'
  }
});

/** The window between the durable final stage and the open fleet
 *  (`onboarding.rs::already_completed`): onboarding questions are settled, the
 *  fleet cannot answer yet, and `not_onboarded` here would send a finished
 *  browser back to the wizard's first screen. */
const startingRefusal = (): AdminResult<unknown> => ({
  Err: {
    kind: 'other',
    message:
      'this Fleet Manager has completed onboarding and is starting; its fleet is not open yet'
  }
});

const stageRefusal = (method: AdminRequestName): AdminResult<unknown> | null => {
  const state = getState();
  if (!state.onboarded) {
    return isOnboardingVerb(method) ? null : notOnboardedRefusal();
  }
  const { stage, runtime } = state.onboarding;
  if (stage === 'holder_authorization' || stage === 'initial_offer') {
    return STAGE_VERBS[stage].has(method) ? null : notOnboardedRefusal();
  }
  if (runtime === 'starting' && method !== 'Onboarding' && !isOnboardingVerb(method)) {
    return startingRefusal();
  }
  // The fleet dispatcher answers these two setup questions with the same
  // refusal the onboard verbs get: they were settled before the fleet existed
  // (admin.rs's RefreshHolderAuthorizations / ConfigureInitialOffer arms).
  if (method === 'RefreshHolderAuthorizations' || method === 'ConfigureInitialOffer') {
    return {
      Err: {
        kind: 'already_onboarded',
        message: 'this Fleet Manager has already been onboarded; a host is set up once'
      }
    };
  }
  return null;
};

/** The one place a payload stops being wire JSON and becomes a declared type.
 *  The daemon has serde here; the mock has this line, and nothing else in the
 *  file guesses at a field name. */
const answer = (method: AdminRequestName, payload: unknown): unknown =>
  verbs[method](payload as never);

export const dispatch = (body: unknown): AdminResult<unknown> => {
  const { method, payload } = parseRequest(body);

  const forced = getState().forcedErrors[method];
  if (forced) return { Err: { kind: 'other', message: forced } };

  if (!isAdminVerb(method)) {
    return {
      Err: {
        kind: 'unparsable_request',
        message: `unparsable admin request: unknown variant ${method}`
      }
    };
  }

  // The durable stage decides which verbs exist right now
  // (crates/fman/core/src/onboarding.rs::answer); a host mid-wizard has no
  // fleet, so every other verb says so rather than inventing an empty one.
  const refusal = stageRefusal(method);
  if (refusal) return refusal;

  try {
    return { Ok: answer(method, payload) };
  } catch (error) {
    return { Err: asAdminError(error) };
  }
};
