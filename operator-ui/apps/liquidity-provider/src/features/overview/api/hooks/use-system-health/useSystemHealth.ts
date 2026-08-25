import type { GetHealthRequest, GetHealthResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { POLL_STANDARD_MS } from '@/shared/api/pollingIntervals';

export const SYSTEM_HEALTH_KEY = ['system-health'] as const;

// Authenticated system-health snapshot for the Overview hub. Polled at the
// standard cadence and fresh for 55s (matching the other section hooks) so a
// route change does not refetch; retry:false surfaces AuthError/NetworkError
// immediately.
export const useSystemHealth = () =>
  useQuery({
    retry: false,
    staleTime: 55_000,
    refetchInterval: POLL_STANDARD_MS,
    queryKey: SYSTEM_HEALTH_KEY,
    refetchOnWindowFocus: true,
    queryFn: () => adminCall<GetHealthRequest, GetHealthResponse>('get_health', null)
  });
