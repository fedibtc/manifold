import type { ScenarioNote } from '@operator-ui/mock-devtools';
import type { FmanVersionReport } from '@operator-ui/types';
import type { MockGuardianFees, MockSeat, MockState } from '@/mocks/state';
import { walletStatus } from '@/mocks/wallet-status';
import { MOCK_HOLDER_PUBKEY, MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';

const SEAT_PRICE_MSAT = 50_000_000;

const FEDERATION_A = 'fed1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const FEDERATION_B = 'fed1bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
const FEDERATION_EMPTY = 'fed1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq';

const DAY_MS = 86_400_000;
// Fixed clock so seat-sale day buckets and remittance timestamps are stable
// across runs — the Overview groups by day, and a moving "today" makes an
// e2e assertion on a bucket label flaky.
const NOW_MS = 1_753_600_000_000;

// `current` is the workspace version the daemon reports from CARGO_PKG_VERSION.
// The default is the healthy case: a publication has been admitted and names the
// running release.
const version = (overrides: Partial<FmanVersionReport> = {}): FmanVersionReport => ({
  current: '0.1.0',
  latest: '0.1.0',
  update_required: false,
  ...overrides
});

// A fixed instant, so a scenario renders the same "last read" line on every run.
// Kept plausibly recent: a mock dashboard showing a check from last year reads as
// a bug to whoever is reviewing the screen.
const LAST_READ_AT = 1_786_000_000;

// The daemon read the relay and found nothing: the ordinary state of a fleet
// waiting for a holder to sign.
const notObserved: MockState['onboarding']['nostr'] = {
  state: 'not_observed',
  checked_at: LAST_READ_AT
};

const authorizationObserved: MockState['onboarding']['nostr'] = {
  state: 'authorization_observed',
  authorizations: 1,
  holders: [MOCK_HOLDER_PUBKEY],
  checked_at: LAST_READ_AT
};

const onboarding = (
  nostr: MockState['onboarding']['nostr'] = notObserved,
  fman_version: FmanVersionReport = version()
): MockState['onboarding'] => ({
  stage: 'complete',
  runtime: 'ready',
  // fman_name and service_pubkey are the values the retired express scenario
  // carried, so the two mock surfaces do not disagree across the migration. The
  // nostr key is not: the express fixture held an npub, which no daemon response
  // can produce.
  fman_name: 'mutual-hamster',
  service_pubkey: '02a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f9',
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr,
  fman_version
});

const authorized = onboarding(authorizationObserved);

const fees = (overrides: Partial<MockGuardianFees> = {}): MockGuardianFees => ({
  federation_id: FEDERATION_A,
  remittance_account: '{"id":"acct1mockguardianfeeaccount"}',
  staged_msat: 0,
  locked_msat: 0,
  idle_msat: 0,
  collected_ecash_msat: 0,
  lifetime_remitted_msat: 0,
  policy: {
    configured: true,
    send_ppm: 1_000,
    recipients: '{"v":1,"recipients":[{"account":"acct1mockguardianfeeaccount","weight":1}]}',
    share_matches_policy: true,
    our_weight: 1,
    total_weight: 4
  },
  remittances: [],
  ...overrides
});

const remittance = (amount_msat: number, daysAgo: number, txid: string) => ({
  amount_msat,
  txid,
  remitted_at_unix: Math.floor((NOW_MS - daysAgo * DAY_MS) / 1000),
  total_msat: amount_msat * 4,
  breakdown: [
    { module: 'ln', direction: 'incoming', amount_msat: Math.floor(amount_msat * 0.7) },
    { module: 'wallet', direction: 'outgoing', amount_msat: Math.ceil(amount_msat * 0.3) }
  ]
});

const seat = (overrides: Partial<MockSeat> & Pick<MockSeat, 'seat_id' | 'report'>): MockSeat => ({
  fi_id: 'fi_02aa11bb22cc33dd44ee55ff66aa77bb88cc99dd00ee11ff22aa33bb44cc55dd',
  plan: { InfiniteBestEffort: { price_msats: SEAT_PRICE_MSAT } },
  created_at_ms: NOW_MS - 3 * DAY_MS,
  payment_claim: { state: 'success', at_ms: NOW_MS - 3 * DAY_MS },
  completion_callback: { state: 'not_configured' },
  decommissioned: false,
  backup: { published_at_ms: NOW_MS - DAY_MS, archive_confirmed: true },
  guardian_fee: {
    remittance_account: '{"id":"acct1mockguardianfeeaccount"}',
    share_matches_policy: true,
    send_ppm: 1_000,
    our_weight: 1,
    total_weight: 4
  },
  fees: fees(),
  ...overrides
});

/** A seat that has no federation yet: both fee surfaces report why rather than
 *  inventing a zero. Mirrors admin.rs::seat_guardian_fee_json before DKG. */
const preFormationSeat = (
  overrides: Partial<MockSeat> & Pick<MockSeat, 'seat_id' | 'report'>
): MockSeat =>
  seat({
    guardian_fee: {
      remittance_account: '{"id":"acct1mockguardianfeeaccount"}',
      policy_error: 'seat has no federation yet'
    },
    fees: undefined,
    ...overrides
  });

const runningSeat = (seat_id: string, invite: string) =>
  seat({
    seat_id,
    report: { state: 'active', health: 'healthy', phase: 'running', invite_code: invite }
  });

const base = (): Pick<
  MockState,
  | 'onboarded'
  | 'authMode'
  | 'sessionActive'
  | 'password'
  | 'latencyMs'
  | 'forcedErrors'
  | 'restoreResult'
  | 'restoreTransport'
  | 'restoreSession'
  | 'onboardingTransport'
  | 'payoutDestination'
  | 'relayAuthorization'
  | 'maxSeats'
  | 'fleetOpensAfterReads'
> => ({
  onboarded: true,
  maxSeats: 3,
  fleetOpensAfterReads: 0,
  relayAuthorization: 'present',
  payoutDestination: 'operator@example.com',
  authMode: 'password',
  sessionActive: false,
  password: 'test-password',
  latencyMs: 0,
  forcedErrors: {},
  restoreResult: 'two-seats-one-formed',
  restoreTransport: 'normal',
  restoreSession: 'active',
  onboardingTransport: 'normal'
});

// `satisfies` rather than a type annotation: it keeps the keys literal, so
// `notes` below can be required to cover exactly this set.
const builders = {
  'fresh-fleet': () => ({
    ...base(),
    seats: [],
    paymentFederations: [],
    price: null,
    onboarding: authorized
  }),
  'not-onboarded': () => ({
    ...base(),
    onboarded: false,
    seats: [],
    paymentFederations: [],
    price: null,
    // A host that has never onboarded has admitted no setup-payment publication,
    // so there is no latest release to compare against yet.
    onboarding: onboarding(undefined, version({ latest: null }))
  }),
  'awaiting-authorization': () => ({
    ...base(),
    relayAuthorization: 'absent',
    seats: [],
    paymentFederations: [],
    price: null,
    onboarding: onboarding()
  }),
  // The window between daemon start and its first relay read. Nothing is known
  // yet, and the Overview deliberately raises no attention item for it.
  'authorization-checking': () => ({
    ...base(),
    relayAuthorization: 'absent',
    seats: [],
    paymentFederations: [],
    price: null,
    onboarding: onboarding({ state: 'checking' })
  }),
  // The relay refused the read. Distinct from an absent authorization: the fleet
  // may or may not be authorized, and the operator is the one who can act.
  'authorization-relay-error': () => ({
    ...base(),
    relayAuthorization: 'absent',
    seats: [],
    paymentFederations: [],
    price: null,
    onboarding: onboarding({
      state: 'relay_error',
      error: 'connect to the configured nostr relay failed: connection refused'
    })
  }),
  'authorization-observed': () => ({
    ...base(),
    seats: [],
    paymentFederations: [],
    price: null,
    onboarding: authorized
  }),
  'authorization-read-error': () => ({
    ...base(),
    seats: [],
    paymentFederations: [],
    price: null,
    onboarding: onboarding(),
    // The relay read failed. The screen must show the error rather than claim the
    // fleet is waiting — those are different facts, and BE-FMAN-AUTH-001 is what
    // would let the daemon distinguish them itself.
    forcedErrors: { Onboarding: 'relay query failed: connection reset' }
  }),
  'fman-update-required': () => ({
    ...base(),
    seats: [],
    paymentFederations: [],
    price: null,
    onboarding: onboarding(
      authorizationObserved,
      version({ latest: '0.2.0', update_required: true })
    )
  }),
  'seats-empty': () => ({
    ...base(),
    seats: [],
    paymentFederations: [
      {
        federation_id: FEDERATION_EMPTY,
        accepted: true,
        receivable: true,
        wallet: walletStatus(0)
      }
    ],
    price: SEAT_PRICE_MSAT,
    onboarding: authorized
  }),
  'seats-mixed': () => ({
    ...base(),
    seats: [
      runningSeat(
        'seat-running-01',
        'fed1running0000000000000000000000000000000000000000000000000000'
      ),
      preFormationSeat({
        seat_id: 'seat-dkg-01',
        payment_claim: { state: 'pending' },
        report: { state: 'active', health: 'healthy', phase: 'dkg_in_progress' }
      }),
      preFormationSeat({
        seat_id: 'seat-created-01',
        payment_claim: { state: 'success', at_ms: NOW_MS - 2 * DAY_MS },
        report: { state: 'active', health: 'unavailable', phase: 'created' }
      }),
      seat({
        seat_id: 'seat-decommissioned-01',
        decommissioned: true,
        report: { state: 'decommissioned', at_ms: NOW_MS - DAY_MS }
      })
    ],
    paymentFederations: [
      {
        federation_id: FEDERATION_A,
        accepted: true,
        receivable: true,
        wallet: walletStatus(250_000_000)
      }
    ],
    price: SEAT_PRICE_MSAT,
    onboarding: authorized
  }),
  'seat-unavailable': () => ({
    ...base(),
    seats: [
      seat({
        seat_id: 'seat-unavailable-01',
        report: {
          state: 'active',
          health: 'unavailable',
          phase: 'running',
          invite_code: 'fed1unavailable00000000000000000000000000000000000000000000000'
        }
      })
    ],
    paymentFederations: [
      {
        federation_id: FEDERATION_A,
        accepted: true,
        receivable: true,
        wallet: walletStatus(100_000_000)
      }
    ],
    price: SEAT_PRICE_MSAT,
    onboarding: authorized
  }),
  'wallet-not-receivable': () => ({
    ...base(),
    seats: [
      runningSeat(
        'seat-running-01',
        'fed1running0000000000000000000000000000000000000000000000000000'
      )
    ],
    paymentFederations: [
      {
        federation_id: FEDERATION_A,
        accepted: true,
        receivable: false,
        wallet: walletStatus(40_000_000)
      }
    ],
    price: SEAT_PRICE_MSAT,
    onboarding: authorized
  }),
  'offer-without-payments': () => ({
    ...base(),
    seats: [],
    paymentFederations: [],
    price: SEAT_PRICE_MSAT,
    onboarding: authorized
  }),
  // The state a fleet is actually in before its first payout: revenue on both
  // sides and nowhere to send it. Every sweep refuses until a destination is
  // stored, which is the ordering the Payouts screen has to make visible.
  'payouts-unset': () => ({
    ...base(),
    payoutDestination: null,
    seats: [
      seat({
        seat_id: 'seat-earning-01',
        report: {
          state: 'active',
          health: 'healthy',
          phase: 'running',
          invite_code: 'fed1earning0000000000000000000000000000000000000000000000000000'
        },
        fees: fees({
          staged_msat: 12_000_000,
          locked_msat: 3_000_000,
          idle_msat: 1_000_000,
          collected_ecash_msat: 0,
          // Nothing has left yet, so the lifetime figure is exactly what the
          // pool still holds.
          lifetime_remitted_msat: 16_000_000
        })
      })
    ],
    paymentFederations: [
      {
        federation_id: FEDERATION_A,
        accepted: true,
        receivable: true,
        wallet: walletStatus(150_000_000)
      }
    ],
    price: SEAT_PRICE_MSAT,
    onboarding: authorized
  }),
  earnings: () => ({
    ...base(),
    seats: [
      seat({
        seat_id: 'seat-earning-01',
        created_at_ms: NOW_MS - 5 * DAY_MS,
        payment_claim: { state: 'success', at_ms: NOW_MS - 5 * DAY_MS },
        report: {
          state: 'active',
          health: 'healthy',
          phase: 'running',
          invite_code: 'fed1earning0000000000000000000000000000000000000000000000000000'
        },
        fees: fees({
          staged_msat: 12_000_000,
          locked_msat: 3_000_000,
          idle_msat: 1_000_000,
          collected_ecash_msat: 8_000_000,
          // More than the three remittances below add up to, and more than the
          // pool holds: earlier money has already been swept out. A total read
          // off the window would be short by the difference.
          lifetime_remitted_msat: 41_500_000,
          remittances: [
            remittance(6_000_000, 0, 'txid-earning-today'),
            remittance(4_000_000, 1, 'txid-earning-yesterday'),
            remittance(6_000_000, 4, 'txid-earning-older')
          ]
        })
      }),
      seat({
        seat_id: 'seat-earning-02',
        created_at_ms: NOW_MS - DAY_MS,
        payment_claim: { state: 'success', at_ms: NOW_MS - DAY_MS },
        report: {
          state: 'active',
          health: 'healthy',
          phase: 'running',
          invite_code: 'fed1earning1111111111111111111111111111111111111111111111111111'
        },
        fees: fees({
          federation_id: FEDERATION_B,
          staged_msat: 2_000_000,
          idle_msat: 500_000,
          collected_ecash_msat: 0,
          lifetime_remitted_msat: 2_500_000,
          remittances: [remittance(2_500_000, 0, 'txid-earning-second')]
        })
      }),
      seat({
        seat_id: 'seat-unpaid-01',
        created_at_ms: NOW_MS - 2 * DAY_MS,
        payment_claim: { state: 'already_spent', at_ms: NOW_MS - 2 * DAY_MS },
        report: { state: 'active', health: 'healthy', phase: 'dkg_in_progress' }
      })
    ],
    paymentFederations: [
      {
        federation_id: FEDERATION_A,
        accepted: true,
        receivable: true,
        wallet: walletStatus(150_000_000)
      },
      {
        federation_id: FEDERATION_B,
        accepted: false,
        receivable: false,
        wallet: walletStatus(12_000_000)
      }
    ],
    price: SEAT_PRICE_MSAT,
    onboarding: authorized
  })
} satisfies Record<string, () => MockState>;

export type ScenarioName = keyof typeof builders;

// Keyed off `builders`, so adding a scenario without documenting it is a type
// error rather than a control panel that silently drifts out of date.
const notes: Record<ScenarioName, ScenarioNote> = {
  'fresh-fleet': {
    desc: 'Default. Onboarded and authorized, but nothing sold yet: no seats, no payment federations, no price.',
    affects: ['overview', 'seats', 'wallet', 'offer']
  },
  'not-onboarded': {
    desc: 'Host has never been onboarded. Only the onboarding verbs answer; everything else refuses.',
    affects: ['setup']
  },
  'awaiting-authorization': {
    desc: 'Onboarded, but no holder has authorized it yet — the QR step is still waiting.',
    affects: ['setup', 'authorization', 'backup', 'overview']
  },
  'authorization-checking': {
    desc: 'The daemon has not finished its first relay read. Nothing is known yet, and the Overview raises no item for it.',
    affects: ['setup', 'authorization', 'overview']
  },
  'authorization-relay-error': {
    desc: 'The relay read failed and nothing is retained. The Overview raises a relay item, not a missing-authorization one.',
    affects: ['setup', 'authorization', 'overview']
  },
  'authorization-observed': {
    desc: 'A holder authorization is on the relay. The Authorization page lists the holder and the full service Nostr public key.',
    affects: ['setup', 'authorization', 'backup', 'overview']
  },
  'authorization-read-error': {
    desc: 'The Onboarding read fails. The authorization state is unknown, and the app shell stays available.',
    affects: ['setup', 'authorization', 'backup', 'overview']
  },
  'fman-update-required': {
    desc: 'The setup-payment publication names a newer FMan release than the one running, so the daemon reports that an update is required. The dashboard takes the whole screen over once, until it is dismissed or the page is reloaded.',
    // Every route inside the shell, because the takeover is mounted in AppShell
    // rather than on a page. `setup` is deliberately absent: setup sits above
    // the shell, and this scenario is onboarded, so the wizard never renders.
    affects: ['overview', 'authorization', 'seats', 'seat-detail', 'wallet', 'offer', 'backup']
  },
  'seats-empty': {
    desc: 'Still no seats, but one receivable federation at a zero balance and a price set.',
    affects: ['seats', 'wallet']
  },
  'seats-mixed': {
    desc: 'Four seats: running, DKG in progress, created, decommissioned. The two pre-formation seats have no fee account yet.',
    affects: ['seats', 'seat-detail', 'overview']
  },
  'seat-unavailable': {
    desc: 'One running seat reporting unavailable health.',
    affects: ['seats', 'overview']
  },
  'wallet-not-receivable': {
    desc: 'Payment federation cannot receive.',
    affects: ['wallet', 'overview']
  },
  'offer-without-payments': {
    desc: 'A paid offer with no payment federation — nothing can ever be bought.',
    affects: ['offer', 'overview']
  },
  'payouts-unset': {
    desc: 'Revenue on both sides and no payout destination stored: one federation holding a balance, one seat with fees in the pool and no collected ecash. Every sweep refuses until a destination is saved.',
    affects: ['payouts']
  },
  earnings: {
    desc: 'Two paid running seats with guardian-fee remittances across several days, one already-spent claim, and a wallet-only leftover federation.',
    affects: ['overview', 'wallet', 'seat-detail', 'payouts']
  }
};

export const scenarioNames = Object.keys(builders) as ScenarioName[];

/** Every scenario with its documentation, for the dev control panel. */
export const scenarioCatalog = scenarioNames.map((name) => ({
  name,
  ...notes[name]
}));

export const hasScenario = (name: string): name is ScenarioName => name in builders;

export const scenario = (name: string): MockState => {
  if (!hasScenario(name)) {
    throw new Error(`unknown scenario: ${name}`);
  }
  return builders[name]();
};
