import type {
  AdminAllocationSummary,
  GetAdvertisementStateResponse,
  GetFundsResponse,
  GetHealthResponse,
  GetSetupStateResponse,
  WalletOperationSummary
} from '@operator-ui/types';
import { describe, expect, it } from 'vitest';
import { deriveOverview, type OverviewInput } from '../derive';

const NOW = 1721476800 + 600; // 10 minutes after the fixtures' observed_at

const readySetup: GetSetupStateResponse = {
  status: 'ready',
  config: null,
  missing_fields: [],
  validation: null
};

const healthyFunds: GetFundsResponse = {
  balance: {
    spendable: 4_200_000,
    pending_incoming: 150_000,
    pending_outgoing: 50_000,
    in_flight_allocations: 800_000,
    fee_reserve: 150_000,
    available_balance: 3_250_000
  },
  replenishment: 'ok',
  gateway: {
    gateway_id: 'gw-signet-01',
    gateway_name: 'Mock Signet Gateway',
    status: 'available',
    available_amount: 3_000_000,
    observed_at: 1721476800
  },
  stability_pool: { status: 'available', available_amount: 250_000, observed_at: 1721476800 },
  effective_liquidity: []
};

const criticalFunds: GetFundsResponse = {
  ...healthyFunds,
  replenishment: 'critical',
  balance: { ...healthyFunds.balance, available_balance: 0 }
};

const publishedAdvertisement: GetAdvertisementStateResponse = {
  advertisement: null,
  publication_status: 'published',
  last_published_at: 1721476800,
  expires_at: null,
  withdrawn_at: null,
  relay_states: [
    { relay_url: 'wss://a', status: 'published', last_error: null, last_seen_at: 1721476800 },
    { relay_url: 'wss://b', status: 'connected', last_error: null, last_seen_at: 1721476800 }
  ],
  ready: true,
  readiness: null,
  unverified_holder_authorization_count: 0
};

const failedAdvertisement: GetAdvertisementStateResponse = {
  ...publishedAdvertisement,
  publication_status: 'failed',
  relay_states: [{ relay_url: 'wss://a', status: 'failed', last_error: 'boom', last_seen_at: null }]
};

const healthyAllocations: AdminAllocationSummary[] = [
  {
    federation_id: 'ft-1',
    gateway_status: 'completed',
    stability_pool_status: null,
    committed_amount: 1_000_000,
    created_at: 1721476800,
    updated_at: 1721476800
  },
  {
    federation_id: 'ft-2',
    gateway_status: 'running',
    stability_pool_status: null,
    committed_amount: 500_000,
    created_at: 1721476800,
    updated_at: 1721476800
  }
];

const allocationsWithFailure: AdminAllocationSummary[] = [
  ...healthyAllocations,
  {
    federation_id: 'ft-3',
    gateway_status: 'failed',
    stability_pool_status: null,
    committed_amount: 750_000,
    created_at: 1721476800,
    updated_at: 1721476800
  }
];

const healthyHealth: GetHealthResponse = {
  overall_status: 'healthy',
  mode: 'normal',
  observed_at: 1721476800,
  components: [
    { component: 'daemon', status: 'healthy', detail: null, observed_at: 1721476800 },
    { component: 'wallet', status: 'healthy', detail: null, observed_at: 1721476800 }
  ]
};

const walletOperations: WalletOperationSummary[] = [
  {
    operation_id: 'wop-0003',
    operation_type: 'deposit',
    amount: 1_000_000,
    status: 'confirmed',
    federation_id: null,
    created_at: 1721476800,
    updated_at: 1721477100
  }
];

const healthyInput: OverviewInput = {
  setup: readySetup,
  funds: healthyFunds,
  advertisement: publishedAdvertisement,
  allocations: healthyAllocations,
  walletOperations,
  health: healthyHealth,
  now: NOW
};

describe('deriveOverview — healthy hub', () => {
  it('should report an all-clear status with no attention items', () => {
    const model = deriveOverview(healthyInput);
    expect(model.status.tone).toBe('healthy');
    expect(model.status.headline).toBe('All systems operational');
    expect(model.attention).toHaveLength(0);
  });

  it('should build the four tiles in order with formatted values', () => {
    const model = deriveOverview(healthyInput);
    expect(model.tiles.map((t) => t.key)).toEqual([
      'balance',
      'advertisement',
      'allocations',
      'health'
    ]);
    expect(model.tiles[0]).toMatchObject({ value: '3,250,000 sats', status: 'healthy' });
    expect(model.tiles[1]).toMatchObject({ value: 'Published', hint: '2/2 relays live' });
    expect(model.tiles[2]).toMatchObject({ value: '2', hint: '1 in progress' });
    expect(model.tiles[3]).toMatchObject({ value: 'Healthy', hint: '2/2 components healthy' });
  });

  it('should surface a recent-activity row and an updated stamp', () => {
    const model = deriveOverview(healthyInput);
    expect(model.activity).toHaveLength(1);
    expect(model.activity[0]).toMatchObject({
      event: 'deposit',
      amount: '1,000,000 sats',
      status: 'confirmed',
      when: '10m ago'
    });
    expect(model.updatedLabel).toBe('Updated 10m ago');
  });
});

describe('deriveOverview — attention hub', () => {
  const attentionInput: OverviewInput = {
    ...healthyInput,
    funds: criticalFunds,
    advertisement: failedAdvertisement,
    allocations: allocationsWithFailure
  };

  it('should escalate to unhealthy with critical attention items', () => {
    const model = deriveOverview(attentionInput);
    expect(model.status.tone).toBe('unhealthy');
    expect(model.status.headline).toBe('Action required');

    const keys = model.attention.map((a) => a.key);
    expect(keys).toContain('funds');
    expect(keys).toContain('advertisement');
    expect(keys).toContain('allocations');

    const funds = model.attention.find((a) => a.key === 'funds');
    expect(funds).toMatchObject({
      severity: 'critical',
      action: { path: '/funds' }
    });
  });

  it('should mark the balance and advertisement tiles as unhealthy', () => {
    const model = deriveOverview(attentionInput);
    expect(model.tiles[0]).toMatchObject({ status: 'unhealthy', hint: 'Critically low' });
    expect(model.tiles[1]).toMatchObject({ value: 'Failed', status: 'unhealthy' });
    expect(model.tiles[2]).toMatchObject({ hint: '1 failed', status: 'unhealthy' });
  });

  it('should downgrade to warning-only when nothing is critical', () => {
    const model = deriveOverview({
      ...healthyInput,
      funds: { ...healthyFunds, replenishment: 'warning' }
    });
    expect(model.status.tone).toBe('warning');
    expect(model.status.headline).toBe('Attention recommended');
    expect(model.attention).toHaveLength(1);
    expect(model.attention[0].severity).toBe('warning');
  });
});

describe('deriveOverview — loading / partial', () => {
  it('should show a checking-status sentence when nothing has loaded', () => {
    const model = deriveOverview({ now: NOW });
    expect(model.status.headline).toBe('Checking system status…');
    expect(model.tiles.every((t) => t.loading)).toBe(true);
    expect(model.updatedLabel).toBeNull();
    expect(model.activity).toHaveLength(0);
  });

  it('should render loaded tiles while others stay in a loading placeholder', () => {
    const model = deriveOverview({ funds: healthyFunds, now: NOW });
    expect(model.tiles[0].loading).toBe(false);
    expect(model.tiles[0].value).toBe('3,250,000 sats');
    expect(model.tiles[3].loading).toBe(true);
    expect(model.tiles[3].value).toBe('—');
    // A single healthy source loaded → no attention, healthy tone.
    expect(model.status.tone).toBe('healthy');
  });
});
