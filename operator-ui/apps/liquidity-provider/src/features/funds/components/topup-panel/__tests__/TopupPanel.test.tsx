import type {
  CreateDepositAddressResponse,
  ListWalletOperationsResponse,
  WalletOperationSummary
} from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('@/shared/api/adminCall', () => ({ adminCall: vi.fn() }));

import { FUNDS_KEY, WALLET_OPERATIONS_KEY } from '@/features/funds/api/hooks/use-funds/useFunds';
import { TopupPanel } from '@/features/funds/components/topup-panel/TopupPanel';
import { adminCall } from '@/shared/api/adminCall';
import { AdminApiError } from '@/shared/api/errors';

const address = 'tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx';

const depositResponse: CreateDepositAddressResponse = {
  address,
  network: 'signet',
  operation_id: 'wop_abc123'
};

const emptyOps: ListWalletOperationsResponse = {
  operations: { items: [], next_page: null }
};

const mockedAdminCall = vi.mocked(adminCall);

const routeMock = (
  deposit: CreateDepositAddressResponse | Error,
  operations: ListWalletOperationsResponse = emptyOps
) => {
  mockedAdminCall.mockImplementation((method: string) => {
    if (method === 'create_deposit_address') {
      return deposit instanceof Error ? Promise.reject(deposit) : Promise.resolve(deposit);
    }
    return Promise.resolve(operations);
  });
};

const renderPanel = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const invalidateSpy = vi.spyOn(client, 'invalidateQueries');
  render(
    <QueryClientProvider client={client}>
      <TopupPanel />
    </QueryClientProvider>
  );
  return invalidateSpy;
};

describe('TopupPanel', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    mockedAdminCall.mockReset();
  });

  it('should request a deposit address on mount and render it in monospace', async () => {
    routeMock(depositResponse);
    renderPanel();

    expect(await screen.findByText(address)).toBeTruthy();
    expect(mockedAdminCall).toHaveBeenCalledWith('create_deposit_address', { label: null });
  });

  it('should state the network and a wrong-network warning', async () => {
    routeMock(depositResponse);
    renderPanel();

    expect(await screen.findByText('signet')).toBeTruthy();
    expect(screen.getByText(/won't arrive/)).toBeTruthy();
  });

  it('should render a QR code encoding the address', async () => {
    routeMock(depositResponse);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { container } = render(
      <QueryClientProvider client={client}>
        <TopupPanel />
      </QueryClientProvider>
    );

    await screen.findByText(address);
    expect(container.querySelector('svg')).toBeTruthy();
  });

  it('should invalidate funds and wallet-operations after creating the address', async () => {
    routeMock(depositResponse);
    const invalidateSpy = renderPanel();

    await screen.findByText(address);
    await waitFor(() => {
      expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: FUNDS_KEY });
      expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: WALLET_OPERATIONS_KEY });
    });
  });

  it('should show "Copied" after a successful clipboard copy', async () => {
    routeMock(depositResponse);
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });

    renderPanel();
    await screen.findByText(address);
    fireEvent.click(screen.getByRole('button', { name: 'Copy address' }));

    expect(await screen.findByRole('button', { name: 'Copied' })).toBeTruthy();
    expect(writeText).toHaveBeenCalledWith(address);
  });

  it('should fall back to selecting the text when the clipboard API fails', async () => {
    routeMock(depositResponse);
    const writeText = vi.fn().mockRejectedValue(new Error('denied'));
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });

    renderPanel();
    await screen.findByText(address);
    fireEvent.click(screen.getByRole('button', { name: 'Copy address' }));

    expect(await screen.findByRole('button', { name: 'Select and copy' })).toBeTruthy();
  });

  it('should disable "New address" while the request is in flight', async () => {
    // First create resolves (panel renders); the retry hangs so isPending stays true.
    let createCalls = 0;
    mockedAdminCall.mockImplementation((method: string) => {
      if (method === 'create_deposit_address') {
        createCalls += 1;
        return createCalls === 1 ? Promise.resolve(depositResponse) : new Promise<never>(() => {});
      }
      return Promise.resolve(emptyOps);
    });

    renderPanel();
    await screen.findByText(address);

    fireEvent.click(screen.getByRole('button', { name: 'New address' }));
    await waitFor(() => {
      const button = screen
        .getAllByRole('button')
        .find((element) => element.textContent === 'New address');
      expect(button?.hasAttribute('disabled')).toBe(true);
    });
  });

  it('should show the "This top-up" watch row with a placeholder until the op appears', async () => {
    routeMock(depositResponse);
    renderPanel();
    await screen.findByText(address);

    expect(screen.getByText('This top-up')).toBeTruthy();
    expect(screen.getByText('wop_abc123')).toBeTruthy();
    expect(screen.getByText('Waiting for deposit')).toBeTruthy();
  });

  it('should advance the watch row status once the op appears in the list', async () => {
    const op: WalletOperationSummary = {
      operation_id: 'wop_abc123',
      operation_type: 'deposit',
      amount: 500_000,
      status: 'broadcast',
      federation_id: null,
      created_at: 1721304000,
      updated_at: 1721304000
    };
    routeMock(depositResponse, { operations: { items: [op], next_page: null } });
    renderPanel();

    expect(await screen.findByText('Broadcast')).toBeTruthy();
    expect(screen.getByText('500,000 sats')).toBeTruthy();
  });

  it('should show an inline error with the code and a retry on failure', async () => {
    routeMock(new AdminApiError('unavailable', 'gateway wallet is down'));
    renderPanel();

    expect(await screen.findByText(/unavailable: gateway wallet is down/)).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Retry' })).toBeTruthy();
  });
});
