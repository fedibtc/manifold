import type { GetHealthResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { NetworkError } from '@/shared/api/errors';
import { POLL_ACTIVE_MS } from '@/shared/api/pollingIntervals';
import { startingReason } from '@/shared/api/restoreMode';

export const HEALTH_KEY = ['health'] as const;

const fetchHealth = async (): Promise<GetHealthResponse> => {
  let response: Response;
  try {
    // Unauthenticated liveness probe. GET (not adminCall, which POSTs to /admin/v1/*).
    response = await fetch('/health');
  } catch {
    throw new NetworkError();
  }

  if (!response.ok) throw new NetworkError(`HTTP ${response.status}`);
  return (await response.json()) as GetHealthResponse;
};

export const useHealth = () =>
  useQuery({
    queryKey: HEALTH_KEY,
    queryFn: fetchHealth,
    retry: false,
    staleTime: 30_000,
    // Polled only while the daemon has no runtime — a live restore swapping the
    // data dir, or a start still building its first generation. This is the one
    // route answering during either, so it is the only thing that can report
    // the wait ending; leaving it unpolled meant the waiting screen cleared
    // whenever the boot query next ran, up to a minute later. A serving daemon
    // keeps the previous behaviour and is not polled from here.
    refetchInterval: (query) => (startingReason(query.state.data) ? POLL_ACTIVE_MS : false)
  });
