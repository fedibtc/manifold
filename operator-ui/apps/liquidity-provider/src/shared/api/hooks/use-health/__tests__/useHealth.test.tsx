import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, vi } from 'vitest';
import { NetworkError } from '../../../errors';
import { useHealth } from '../useHealth';

const originalFetch = global.fetch;

const jsonResponse = (body: unknown, init?: ResponseInit): Response =>
  new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'content-type': 'application/json' },
    ...init
  });

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

afterEach(() => {
  global.fetch = originalFetch;
  vi.restoreAllMocks();
});

const healthyResponse = {
  overall_status: 'healthy',
  observed_at: 1721476800,
  components: [{ name: 'daemon', status: 'healthy', detail: null, observed_at: 1721476800 }]
};

it('should GET /health and return the parsed GetHealthResponse on 2xx', async () => {
  const fetchMock = vi.fn().mockResolvedValue(jsonResponse(healthyResponse));
  global.fetch = fetchMock;

  const { result } = renderHook(() => useHealth(), { wrapper });

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(result.current.data).toEqual(healthyResponse);
  expect(fetchMock).toHaveBeenCalledWith('/health');
});

it('should surface a NetworkError when the daemon is unreachable', async () => {
  global.fetch = vi.fn().mockRejectedValue(new TypeError('fetch failed'));

  const { result } = renderHook(() => useHealth(), { wrapper });

  await waitFor(() => expect(result.current.isError).toBe(true));
  expect(result.current.error).toBeInstanceOf(NetworkError);
});

it('should surface a NetworkError on a non-2xx response', async () => {
  global.fetch = vi.fn().mockResolvedValue(jsonResponse({}, { status: 500 }));

  const { result } = renderHook(() => useHealth(), { wrapper });

  await waitFor(() => expect(result.current.isError).toBe(true));
  expect(result.current.error).toBeInstanceOf(NetworkError);
});
