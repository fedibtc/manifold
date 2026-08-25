import type { ScenarioNote } from '@operator-ui/mock-devtools';
import {
  advertisementExpiresAt,
  advertisementIssuedAt,
  advertisementStaleExpiresAt,
  allocationDetails,
  allocationSummaries,
  criticalFunds,
  degradedHealth,
  failedValidation,
  freshMissingFields,
  healthyFunds,
  healthyHealth,
  passedValidation,
  publishedRelayStates,
  readyAdvertisement,
  readySetupConfigView,
  seededAttestations,
  staleRelayStates,
  walletOperationsPage,
  withdrawnRelayStates
} from '@operator-ui/mock-fixtures';
import type { StoredSecrets } from '@/mocks/logic';
import type { MockState } from '@/mocks/state';

const noAllocations = (): MockState['allocations'] => ({ summaries: [], details: {} });

const seededAllocations = (): MockState['allocations'] => ({
  summaries: structuredClone(allocationSummaries),
  details: structuredClone(allocationDetails)
});

const notReadyAdvertisement = (): MockState['advertisement'] => ({
  publicationStatus: 'not_ready',
  ready: false,
  view: null,
  relayStates: [],
  lastPublishedAt: null,
  expiresAt: null,
  withdrawnAt: null
});

const publishedAdvertisement = (): MockState['advertisement'] => ({
  publicationStatus: 'published',
  ready: true,
  view: structuredClone(readyAdvertisement),
  relayStates: structuredClone(publishedRelayStates),
  lastPublishedAt: advertisementIssuedAt,
  expiresAt: advertisementExpiresAt,
  withdrawnAt: null
});

// A configured provider has both secrets stored; the mock keeps the same two
// presence flags the daemon derives from its secret store.
const CONFIGURED_SECRETS: StoredSecrets = {
  gateway_admin_credential: true,
  chain_observer_password: true
};

const NO_SECRETS: StoredSecrets = {
  gateway_admin_credential: false,
  chain_observer_password: false
};

const readySetup = (): MockState['setup'] => ({
  status: 'ready',
  config: structuredClone(readySetupConfigView),
  draft: null,
  missingFields: [],
  validation: structuredClone(passedValidation),
  secrets: { ...CONFIGURED_SECRETS }
});

const noAttestations = (): NonNullable<MockState['attestations']> => [];

const seededAttestationList = (): NonNullable<MockState['attestations']> =>
  structuredClone(seededAttestations);

// Each factory returns a fresh MockState so callers never share references.
// `satisfies` rather than a type annotation: it keeps the keys literal, so
// `notes` below can be required to cover exactly this set.
const builders = {
  'setup-fresh': () => ({
    setup: {
      status: 'not_configured',
      config: null,
      draft: null,
      missingFields: [...freshMissingFields],
      validation: null,
      secrets: { ...NO_SECRETS }
    },
    advertisement: notReadyAdvertisement(),
    funds: structuredClone(healthyFunds),
    health: structuredClone(healthyHealth),
    walletOperations: structuredClone(walletOperationsPage),
    allocations: noAllocations(),
    attestations: noAttestations(),
    phase: 11,
    bootMode: 'normal',
    latencyMs: 0,
    forcedErrors: {}
  }),
  'setup-pending': () => ({
    setup: {
      status: 'pending_validation',
      config: null,
      draft: null,
      missingFields: [...freshMissingFields],
      validation: structuredClone(failedValidation),
      secrets: { ...NO_SECRETS }
    },
    advertisement: notReadyAdvertisement(),
    funds: structuredClone(healthyFunds),
    health: structuredClone(healthyHealth),
    walletOperations: structuredClone(walletOperationsPage),
    allocations: noAllocations(),
    attestations: noAttestations(),
    phase: 11,
    bootMode: 'normal',
    latencyMs: 0,
    forcedErrors: {}
  }),
  'all-clear': () => ({
    setup: readySetup(),
    advertisement: publishedAdvertisement(),
    funds: structuredClone(healthyFunds),
    health: structuredClone(healthyHealth),
    walletOperations: structuredClone(walletOperationsPage),
    allocations: noAllocations(),
    attestations: seededAttestationList(),
    phase: 11,
    bootMode: 'normal',
    latencyMs: 0,
    forcedErrors: {}
  }),
  'funds-critical': () => ({
    setup: readySetup(),
    advertisement: publishedAdvertisement(),
    funds: structuredClone(criticalFunds),
    health: structuredClone(healthyHealth),
    walletOperations: structuredClone(walletOperationsPage),
    allocations: noAllocations(),
    attestations: seededAttestationList(),
    phase: 11,
    bootMode: 'normal',
    latencyMs: 0,
    forcedErrors: {}
  }),
  'ad-stale': () => ({
    setup: readySetup(),
    advertisement: {
      publicationStatus: 'stale',
      ready: true,
      view: structuredClone(readyAdvertisement),
      relayStates: structuredClone(staleRelayStates),
      lastPublishedAt: advertisementIssuedAt,
      expiresAt: advertisementStaleExpiresAt,
      withdrawnAt: null
    },
    funds: structuredClone(healthyFunds),
    health: structuredClone(healthyHealth),
    walletOperations: structuredClone(walletOperationsPage),
    allocations: noAllocations(),
    attestations: seededAttestationList(),
    phase: 11,
    bootMode: 'normal',
    latencyMs: 0,
    forcedErrors: {}
  }),
  'ad-withdrawn': () => ({
    setup: readySetup(),
    advertisement: {
      publicationStatus: 'withdrawn',
      ready: true,
      view: null,
      relayStates: structuredClone(withdrawnRelayStates),
      lastPublishedAt: advertisementIssuedAt,
      expiresAt: null,
      withdrawnAt: advertisementIssuedAt
    },
    funds: structuredClone(healthyFunds),
    health: structuredClone(healthyHealth),
    walletOperations: structuredClone(walletOperationsPage),
    allocations: noAllocations(),
    attestations: seededAttestationList(),
    phase: 11,
    bootMode: 'normal',
    latencyMs: 0,
    forcedErrors: {}
  }),
  // Ready daemon whose system health is degraded (chain observer unhealthy,
  // wallet warning) so the Overview surfaces the degraded-components state.
  'health-degraded': () => ({
    setup: readySetup(),
    advertisement: publishedAdvertisement(),
    funds: structuredClone(healthyFunds),
    health: structuredClone(degradedHealth),
    walletOperations: structuredClone(walletOperationsPage),
    allocations: noAllocations(),
    attestations: seededAttestationList(),
    phase: 11,
    bootMode: 'normal',
    latencyMs: 0,
    forcedErrors: {}
  }),
  // Ready daemon whose funds report the 'warning' replenishment band, so /funds
  // shows the amber banner and 'Below warning threshold' chip. Balance is left
  // healthy on purpose: the UI derives banner/chip purely from the backend
  // `replenishment` field and never recomputes from thresholds.
  'funds-warning': () => ({
    setup: readySetup(),
    advertisement: publishedAdvertisement(),
    funds: { ...structuredClone(healthyFunds), replenishment: 'warning' },
    health: structuredClone(healthyHealth),
    walletOperations: structuredClone(walletOperationsPage),
    allocations: noAllocations(),
    attestations: seededAttestationList(),
    phase: 11,
    bootMode: 'normal',
    latencyMs: 0,
    forcedErrors: {}
  }),
  // Ready daemon whose recent wallet operations exercise the 'broadcast' and
  // 'cancelled' statuses. WalletOperationsTable renders each status via
  // humanizeToken (snake_case -> spaced). Index mutation keeps the literal
  // assignable to WalletOperationStatus without widening to string.
  'wallet-ops-broadcast-cancelled': () => {
    const walletOperations = structuredClone(walletOperationsPage);
    walletOperations[0].status = 'broadcast';
    walletOperations[1].status = 'cancelled';
    return {
      setup: readySetup(),
      advertisement: publishedAdvertisement(),
      funds: structuredClone(healthyFunds),
      health: structuredClone(healthyHealth),
      walletOperations,
      allocations: noAllocations(),
      attestations: seededAttestationList(),
      phase: 11,
      bootMode: 'normal',
      latencyMs: 0,
      forcedErrors: {}
    };
  },
  // Ready daemon whose recent wallet operations exercise the two 'needs review'
  // statuses ('in_doubt', 'manual_review_required'). The always-visible
  // WalletOperationsTable humanizes the token, so these render as 'in doubt' and
  // 'manual review required' on /funds.
  'wallet-ops-review': () => {
    const walletOperations = structuredClone(walletOperationsPage);
    walletOperations[0].status = 'in_doubt';
    walletOperations[1].status = 'manual_review_required';
    return {
      setup: readySetup(),
      advertisement: publishedAdvertisement(),
      funds: structuredClone(healthyFunds),
      health: structuredClone(healthyHealth),
      walletOperations,
      allocations: noAllocations(),
      attestations: seededAttestationList(),
      phase: 11,
      bootMode: 'normal',
      latencyMs: 0,
      forcedErrors: {}
    };
  },
  // Ready daemon whose advertisement failed to publish. Keeps the signed view so
  // the listing card still renders; header chip reads 'Failed'.
  'ad-failed': () => ({
    setup: readySetup(),
    advertisement: {
      publicationStatus: 'failed',
      ready: true,
      view: structuredClone(readyAdvertisement),
      relayStates: structuredClone(publishedRelayStates),
      lastPublishedAt: advertisementIssuedAt,
      expiresAt: advertisementExpiresAt,
      withdrawnAt: null
    },
    funds: structuredClone(healthyFunds),
    health: structuredClone(healthyHealth),
    walletOperations: structuredClone(walletOperationsPage),
    allocations: noAllocations(),
    attestations: seededAttestationList(),
    phase: 11,
    bootMode: 'normal',
    latencyMs: 0,
    forcedErrors: {}
  }),
  // Published advertisement whose relay table mixes a healthy 'connected' relay
  // (label 'Connected') with a 'failed' relay whose last_error is appended after
  // ' · '. Publication stays 'published' so the header chip does not collide with
  // the relay 'Failed' label.
  'ad-relays-mixed': () => ({
    setup: readySetup(),
    advertisement: {
      publicationStatus: 'published',
      ready: true,
      view: structuredClone(readyAdvertisement),
      relayStates: [
        {
          relay_url: 'wss://relay.connected.example',
          status: 'connected',
          last_error: null,
          last_seen_at: advertisementIssuedAt
        },
        {
          relay_url: 'wss://relay.failed.example',
          status: 'failed',
          last_error: 'relay handshake rejected',
          last_seen_at: advertisementIssuedAt
        }
      ],
      lastPublishedAt: advertisementIssuedAt,
      expiresAt: advertisementExpiresAt,
      withdrawnAt: null
    },
    funds: structuredClone(healthyFunds),
    health: structuredClone(healthyHealth),
    walletOperations: structuredClone(walletOperationsPage),
    allocations: noAllocations(),
    attestations: seededAttestationList(),
    phase: 11,
    bootMode: 'normal',
    latencyMs: 0,
    forcedErrors: {}
  }),
  // Ready daemon whose fed-0001 allocation collapses to 'action_required' on both
  // the summary (gateway_status) and the detail (item_statuses[0]);
  // action_required is in CANCELLABLE_STATUSES so the timeline offers a cancel.
  'allocations-action-required': () => {
    const allocations = seededAllocations();
    const target = allocations.summaries[0];
    target.gateway_status = 'action_required';
    allocations.details[target.federation_id].status.item_statuses[0].status = 'action_required';
    return {
      setup: readySetup(),
      advertisement: publishedAdvertisement(),
      funds: structuredClone(healthyFunds),
      health: structuredClone(healthyHealth),
      walletOperations: structuredClone(walletOperationsPage),
      allocations,
      attestations: seededAttestationList(),
      phase: 11,
      bootMode: 'normal',
      latencyMs: 0,
      forcedErrors: {}
    };
  },
  // Ready daemon whose fed-0001 allocation collapses to 'cancelled' on both the
  // summary (gateway_status) and the detail (item_statuses[0]); cancelled is NOT
  // in CANCELLABLE_STATUSES so no cancel affordance renders.
  'allocations-cancelled': () => {
    const allocations = seededAllocations();
    const target = allocations.summaries[0];
    target.gateway_status = 'cancelled';
    allocations.details[target.federation_id].status.item_statuses[0].status = 'cancelled';
    return {
      setup: readySetup(),
      advertisement: publishedAdvertisement(),
      funds: structuredClone(healthyFunds),
      health: structuredClone(healthyHealth),
      walletOperations: structuredClone(walletOperationsPage),
      allocations,
      attestations: seededAttestationList(),
      phase: 11,
      bootMode: 'normal',
      latencyMs: 0,
      forcedErrors: {}
    };
  },
  // Ready daemon with a mix of pending/running/completed/failed allocations so
  // the /allocations screen and its inline timeline have data to render.
  'allocations-mixed': () => ({
    setup: readySetup(),
    advertisement: publishedAdvertisement(),
    funds: structuredClone(healthyFunds),
    health: structuredClone(healthyHealth),
    walletOperations: structuredClone(walletOperationsPage),
    allocations: seededAllocations(),
    attestations: seededAttestationList(),
    phase: 11,
    bootMode: 'normal',
    latencyMs: 0,
    forcedErrors: {}
  }),
  // Authenticated operator token whose get_setup_state calls the daemon
  // rejects as permission_denied (403) — the access-denied boot state, never
  // the re-auth prompt (see SPEC-flip-admin-api.md:31-33).
  'access-denied': () => ({
    setup: readySetup(),
    advertisement: publishedAdvertisement(),
    funds: structuredClone(healthyFunds),
    health: structuredClone(healthyHealth),
    walletOperations: structuredClone(walletOperationsPage),
    allocations: noAllocations(),
    attestations: seededAttestationList(),
    phase: 11,
    bootMode: 'normal',
    latencyMs: 0,
    forcedErrors: { get_setup_state: 'permission_denied' }
  })
} satisfies Record<string, () => MockState>;

export type ScenarioName = keyof typeof builders;

// Keyed off `builders`, so adding a scenario without documenting it is a type
// error rather than a control panel that silently drifts out of date.
const notes: Record<ScenarioName, ScenarioNote> = {
  'setup-fresh': {
    desc: 'Default. Nothing configured yet: the wizard is the only reachable screen.',
    affects: ['setup']
  },
  'setup-pending': {
    desc: "Setup status is 'pending_validation': config is still unset, but a previous validation attempt already failed (gateway unreachable, no relays configured). The wizard stays gated.",
    affects: ['setup', 'settings']
  },
  'all-clear': {
    desc: 'Fully configured and published: healthy funds, a live advertisement, relays reporting published.',
    affects: ['overview', 'funds', 'advertisement', 'settings']
  },
  'funds-critical': {
    desc: 'Balance below the critical threshold.',
    affects: ['overview', 'funds']
  },
  'funds-warning': {
    desc: "Replenishment flagged 'warning' while the balance itself stays healthy — the UI reads the banner and chip from the replenishment field, not the raw numbers.",
    affects: ['overview', 'funds']
  },
  'ad-stale': {
    desc: 'The advertisement has expired and needs republishing.',
    affects: ['overview', 'advertisement']
  },
  'ad-withdrawn': {
    desc: 'The advertisement has been withdrawn; the provider is not discoverable.',
    affects: ['overview', 'advertisement']
  },
  'ad-failed': {
    desc: "Publication failed overall (header chip reads 'Failed'), though the relay table still shows the prior successful publications.",
    affects: ['overview', 'advertisement']
  },
  'ad-relays-mixed': {
    desc: 'Some relays accepted the advertisement and some rejected it.',
    affects: ['advertisement']
  },
  'health-degraded': {
    desc: 'One or more components report degraded health.',
    affects: ['overview']
  },
  'wallet-ops-broadcast-cancelled': {
    desc: "Two recent wallet operations: one shows status 'broadcast', another shows 'cancelled'.",
    affects: ['funds']
  },
  'wallet-ops-review': {
    desc: "Two recent wallet operations need review: one 'in doubt', one 'manual review required'.",
    affects: ['funds']
  },
  'allocations-action-required': {
    desc: 'An allocation is stalled and needs the operator to act.',
    affects: ['overview', 'allocations']
  },
  'allocations-cancelled': {
    desc: 'An allocation was cancelled.',
    affects: ['allocations']
  },
  'allocations-mixed': {
    desc: 'Allocations spanning four states: completed, running, pending, and failed.',
    affects: ['allocations']
  },
  'access-denied': {
    desc: 'Authenticated, but get_setup_state is rejected as permission_denied (403) — the access-denied boot screen, not the re-auth prompt.',
    affects: ['setup']
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
