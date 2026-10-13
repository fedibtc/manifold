import type { ShowSeatReadinessResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';

export const SEAT_READINESS_KEY = ['seat-readiness'] as const;

/** The daemon's latest new-seat readiness run. Polled, because the daemon
 *  reruns it on its own schedule and a failing check retries every minute. */
export const useSeatReadiness = () =>
  useQuery({
    queryKey: SEAT_READINESS_KEY,
    queryFn: () => adminCall<ShowSeatReadinessResponse>('ShowSeatReadiness'),
    refetchInterval: 30_000
  });
