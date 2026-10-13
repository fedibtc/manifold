import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { useShowMnemonic } from '../useShowMnemonic';

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

// The cache eviction cases need the client the hook actually used, so they render
// against one this helper holds rather than the one `wrapper` makes per render.
const renderAgainstOwnClient = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const ownClientWrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return { ...renderHook(() => useShowMnemonic(), { wrapper: ownClientWrapper }), client };
};

afterEach(() => {
  vi.restoreAllMocks();
});

it('should not call adminCall until mutate is invoked', () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ mnemonic: 'a b c' });

  renderHook(() => useShowMnemonic(), { wrapper });

  expect(adminCallSpy).not.toHaveBeenCalled();
});

it('should call adminCall with ShowMnemonic and return the phrase on mutate', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ mnemonic: 'a b c' });

  const { result } = renderHook(() => useShowMnemonic(), { wrapper });
  result.current.mutate();

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(adminCallSpy).toHaveBeenCalledWith('ShowMnemonic');
  expect(result.current.data).toEqual({ mnemonic: 'a b c' });
});

it('should drop the phrase from the mutation cache as soon as nothing observes it', async () => {
  // Without gcTime: 0 the settled mutation — the fleet's root mnemonic and all —
  // stays in the MutationCache for the default five minutes after the last
  // observer goes.
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ mnemonic: 'a b c' });

  const { result, client, unmount } = renderAgainstOwnClient();
  result.current.mutate();

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(client.getMutationCache().getAll()).toHaveLength(1);

  unmount();

  await waitFor(() => expect(client.getMutationCache().getAll()).toHaveLength(0));
});

it('should keep no copy of the phrase once the mutation is reset', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ mnemonic: 'a b c' });

  const { result, client } = renderAgainstOwnClient();
  result.current.mutate();

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  result.current.reset();

  await waitFor(() => expect(result.current.data).toBeUndefined());
  await waitFor(() => expect(client.getMutationCache().getAll()).toHaveLength(0));
});
