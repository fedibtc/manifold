import type {
  GetFundsResponse,
  ListWalletOperationsResponse,
  WalletOperationSummary
} from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as fundsHooks from '@/features/funds/api/hooks/use-funds/useFunds';
import * as walletOpsHooks from '@/features/funds/api/hooks/use-wallet-operations/useWalletOperations';
import { AuthError } from '@/shared/api/errors';
import { FundsPage } from '../FundsPage';

const renderPage = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <FundsPage />
    </QueryClientProvider>
  );
};

type FundsResult = ReturnType<typeof fundsHooks.useFunds>;
type WalletOpsResult = ReturnType<typeof walletOpsHooks.useWalletOperations>;

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
  effective_liquidity: [{ source_type: 'gateway', gateway_id: 'gw-signet-01', amount: 3_000_000 }]
};

const criticalFunds: GetFundsResponse = {
  ...healthyFunds,
  replenishment: 'critical',
  balance: { ...healthyFunds.balance, available_balance: 0 }
};

const walletOperationsPage: WalletOperationSummary[] = [
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

const walletOpsData: ListWalletOperationsResponse = {
  operations: { items: walletOperationsPage, next_page: null }
};

const asFundsResult = (partial: Partial<FundsResult>): FundsResult =>
  partial as unknown as FundsResult;

const asWalletOpsResult = (partial: Partial<WalletOpsResult>): WalletOpsResult =>
  partial as unknown as WalletOpsResult;

const mockWalletOperations = (): void => {
  vi.spyOn(walletOpsHooks, 'useWalletOperations').mockReturnValue(
    asWalletOpsResult({ isSuccess: true, data: walletOpsData })
  );
};

describe('FundsPage', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('should render a loading placeholder while funds are fetching', () => {
    vi.spyOn(fundsHooks, 'useFunds').mockReturnValue(asFundsResult({ isLoading: true }));
    mockWalletOperations();

    renderPage();

    expect(screen.getByText('Loading funds…')).toBeTruthy();
  });

  it('should render an error banner when the funds query fails', () => {
    vi.spyOn(fundsHooks, 'useFunds').mockReturnValue(
      asFundsResult({ isError: true, error: new AuthError() })
    );
    mockWalletOperations();

    renderPage();

    expect(screen.getByText("Couldn't load funds")).toBeTruthy();
  });

  it('should render balances, sources and operations for a healthy snapshot', () => {
    vi.spyOn(fundsHooks, 'useFunds').mockReturnValue(
      asFundsResult({ isSuccess: true, data: healthyFunds })
    );
    mockWalletOperations();

    renderPage();

    expect(screen.getAllByText('3,250,000 sats').length).toBeGreaterThan(0);
    expect(screen.getByText('Mock Signet Gateway')).toBeTruthy();
    expect(screen.getByText('wop-0003')).toBeTruthy();
    expect(screen.queryByText('Critical balance')).toBeNull();
  });

  it('should render the critical banner for a critical snapshot', () => {
    vi.spyOn(fundsHooks, 'useFunds').mockReturnValue(
      asFundsResult({ isSuccess: true, data: criticalFunds })
    );
    mockWalletOperations();

    renderPage();

    expect(screen.getByText('Critical balance')).toBeTruthy();
  });

  it('should keep showing balances with a stale banner when a poll tick fails but cached data remains', () => {
    vi.spyOn(fundsHooks, 'useFunds').mockReturnValue(
      asFundsResult({
        isError: true,
        error: new AuthError(),
        data: healthyFunds,
        dataUpdatedAt: 1721476800000
      })
    );
    mockWalletOperations();

    renderPage();

    expect(screen.getAllByText('3,250,000 sats').length).toBeGreaterThan(0);
    expect(screen.getByText(/showing last-known data/i)).toBeTruthy();
    expect(screen.getByText(/last updated/i)).toBeTruthy();
    expect(screen.queryByText("Couldn't load funds")).toBeNull();
  });
});
