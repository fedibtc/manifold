import type { ShowReachResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { REACH_POLL_MS } from '@/shared/api/pollingIntervals';

export const REACH_KEY = ['reach'] as const;

/** Guardian-fee call timings and each seat's view of its fellow guardians. */
export const useReach = () =>
  useQuery({
    queryKey: REACH_KEY,
    queryFn: () => adminCall<ShowReachResponse>('ShowReach'),
    refetchInterval: REACH_POLL_MS
  });
