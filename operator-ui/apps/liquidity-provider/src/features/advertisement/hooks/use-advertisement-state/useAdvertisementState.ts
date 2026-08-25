import type { GetAdvertisementStateResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { POLL_STANDARD_MS } from '@/shared/api/pollingIntervals';
import { ADVERTISEMENT_KEY } from '@/shared/api/queryKeys';

// Publication state for the /advertisement screen. Polled at the standard
// cadence so a route change stays fresh; retry:false surfaces
// AuthError/NetworkError immediately.
export const useAdvertisementState = () =>
  useQuery({
    retry: false,
    staleTime: 55_000,
    refetchInterval: POLL_STANDARD_MS,
    queryKey: ADVERTISEMENT_KEY,
    refetchOnWindowFocus: true,
    queryFn: () => adminCall<null, GetAdvertisementStateResponse>('get_advertisement_state', null)
  });
