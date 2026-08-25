import type { QueryRead } from '@operator-ui/common-ui';
import type {
  AdminAllocationSummary,
  GetAdvertisementStateResponse,
  GetFundsResponse,
  GetHealthResponse,
  GetSetupStateResponse,
  ListAllocationsResponse,
  ListWalletOperationsResponse,
  WalletOperationSummary
} from '@operator-ui/types';
import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as advertisementHooks from '@/features/advertisement/hooks/use-advertisement-state/useAdvertisementState';
import * as allocationsHooks from '@/features/allocations/api/hooks/use-allocations/useAllocations';
import * as fundsHooks from '@/features/funds/api/hooks/use-funds/useFunds';
import * as walletOpsHooks from '@/features/funds/api/hooks/use-wallet-operations/useWalletOperations';
import * as healthHooks from '@/features/overview/api/hooks/use-system-health/useSystemHealth';
import * as setupHooks from '@/shared/api/hooks/use-setup-state/useSetupState';
import { OverviewPage } from '../OverviewPage';

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
    { relay_url: 'wss://a', status: 'published', last_error: null, last_seen_at: 1721476800 }
  ],
  ready: true,
  readiness: null,
  unverified_holder_authorization_count: 0
};

const failedAdvertisement: GetAdvertisementStateResponse = {
  ...publishedAdvertisement,
  publication_status: 'failed'
};

const allocations: AdminAllocationSummary[] = [
  {
    federation_id: 'ft-1',
    gateway_status: 'completed',
    stability_pool_status: null,
    committed_amount: 1_000_000,
    created_at: 1721476800,
    updated_at: 1721476800
  }
];

const healthyHealth: GetHealthResponse = {
  overall_status: 'healthy',
  mode: 'normal',
  observed_at: 1721476800,
  components: [{ component: 'daemon', status: 'healthy', detail: null, observed_at: 1721476800 }]
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

const allocationsResponse: ListAllocationsResponse = {
  allocations: { items: allocations, next_page: null }
};

const walletOpsResponse: ListWalletOperationsResponse = {
  operations: { items: walletOperations, next_page: null }
};

const asData = <T,>(data: T | undefined, read: Partial<QueryRead> = {}) =>
  ({ data, refetch: () => {}, ...read }) as unknown as never;

interface MockOptions {
  funds?: GetFundsResponse;
  advertisement?: GetAdvertisementStateResponse;
  loading?: boolean;
  // Applied to the health read only: one failing read is enough to mark the
  // whole surface, which is the property under test.
  healthRead?: Partial<QueryRead>;
}

const mockHooks = (options: MockOptions = {}): void => {
  const loading = options.loading ?? false;
  vi.spyOn(setupHooks, 'useSetupState').mockReturnValue(asData(loading ? undefined : readySetup));
  vi.spyOn(fundsHooks, 'useFunds').mockReturnValue(
    asData(loading ? undefined : (options.funds ?? healthyFunds))
  );
  vi.spyOn(walletOpsHooks, 'useWalletOperations').mockReturnValue(
    asData(loading ? undefined : walletOpsResponse)
  );
  vi.spyOn(advertisementHooks, 'useAdvertisementState').mockReturnValue(
    asData(loading ? undefined : (options.advertisement ?? publishedAdvertisement))
  );
  vi.spyOn(allocationsHooks, 'useAllocations').mockReturnValue(
    asData(loading ? undefined : allocationsResponse)
  );
  vi.spyOn(healthHooks, 'useSystemHealth').mockReturnValue(
    asData(loading ? undefined : healthyHealth, options.healthRead)
  );
};

const renderPage = () =>
  render(
    <MemoryRouter initialEntries={['/']}>
      <OverviewPage />
    </MemoryRouter>
  );

describe('OverviewPage', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('should render an all-clear hub with tiles, quick actions and activity', () => {
    mockHooks();

    renderPage();

    expect(screen.getByRole('heading', { name: 'Overview' })).toBeTruthy();
    expect(screen.getByText('All systems operational')).toBeTruthy();
    expect(screen.getByText('3,250,000 sats')).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Funds' })).toBeTruthy();
    expect(screen.getByText('deposit')).toBeTruthy();
    expect(screen.queryByText('Needs attention')).toBeNull();
  });

  it('should surface attention items for a critical fund and a failed advertisement', () => {
    mockHooks({ funds: criticalFunds, advertisement: failedAdvertisement });

    renderPage();

    expect(screen.getByText('Action required')).toBeTruthy();
    expect(screen.getByText('Needs attention')).toBeTruthy();
    expect(screen.getByText('Available balance critically low')).toBeTruthy();
    expect(screen.getByText('Advertisement failed to publish')).toBeTruthy();
  });

  // Previously this rendered the full hub — a status banner reading "Checking
  // system status…", tiles of em dashes, and "No recent activity yet." — which
  // states things about a daemon that has not answered. Nothing has been said
  // yet, so the page says nothing yet.
  it('should claim nothing while every source is still fetching', () => {
    mockHooks({ loading: true });

    renderPage();

    expect(screen.getByText('Loading…')).toBeTruthy();
    expect(screen.queryByText('No recent activity yet.')).toBeNull();
    expect(screen.queryByText('—')).toBeNull();
  });

  // The defect this screen was named for. `deriveStatus` answers "All systems
  // operational" whenever one read holds data and no attention item fires, and
  // the page never looked at `isError` — so a dead daemon rendered green.
  it('should not report healthy when a read failed and nothing was held', () => {
    const refetch = vi.fn();
    mockHooks({
      loading: true,
      healthRead: { isError: true, error: new Error('daemon unreachable'), refetch }
    });

    renderPage();

    expect(screen.queryByText('All systems operational')).toBeNull();
    expect(screen.getByText('daemon unreachable')).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(refetch).toHaveBeenCalled();
  });

  // A failed refresh on top of good figures. The figures are worth more than a
  // blank screen, so they stay — under a marker saying how old they are.
  it('should keep the last known figures under a staleness marker', () => {
    mockHooks({
      healthRead: {
        isError: true,
        error: new Error('refresh failed'),
        dataUpdatedAt: 1_700_000_000_000
      }
    });

    renderPage();

    expect(screen.getByText('Showing last-known data')).toBeTruthy();
    expect(screen.getByText('3,250,000 sats')).toBeTruthy();
  });
});
