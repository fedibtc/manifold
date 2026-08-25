import type { ListWalletOperationsResponse, WalletOperationSummary } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { useWalletOperations, walletOperationsInterval } from '../useWalletOperations';

const walletOperations: WalletOperationSummary[] = [
  {
    operation_id: 'wop-0001',
    operation_type: 'withdrawal',
    amount: 250_000,
    status: 'pending',
    federation_id: null,
    created_at: 1721304000,
    updated_at: 1721304000
  }
];

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

afterEach(() => {
  vi.restoreAllMocks();
});

it('should call list_wallet_operations with a page limit and return the list', async () => {
  const response: ListWalletOperationsResponse = {
    operations: { items: walletOperations, next_page: null }
  };
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(response);

  const { result } = renderHook(() => useWalletOperations(), { wrapper });

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(result.current.data).toEqual(response);
  expect(adminCallSpy).toHaveBeenCalledWith('list_wallet_operations', { page: { limit: 50 } });
});

it('should poll fast while a deposit watch is armed', () => {
  expect(walletOperationsInterval(undefined, true)).toBe(5_000);
});

it('should poll fast while any visible op is pending or broadcast', () => {
  expect(walletOperationsInterval(walletOperations, false)).toBe(5_000);
  const broadcast = [{ ...walletOperations[0], status: 'broadcast' as const }];
  expect(walletOperationsInterval(broadcast, false)).toBe(5_000);
});

it('should poll idle when nothing is being watched or settling', () => {
  const settled = [{ ...walletOperations[0], status: 'completed' as const }];
  expect(walletOperationsInterval(settled, false)).toBe(30_000);
  expect(walletOperationsInterval(undefined, false)).toBe(30_000);
});
