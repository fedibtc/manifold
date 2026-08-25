import type { ListSeatsResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { LIST_POLL_MS } from '@/shared/api/pollingIntervals';

export const SEATS_KEY = ['seats'] as const;

export const useSeats = () =>
  useQuery({
    queryKey: SEATS_KEY,
    refetchInterval: LIST_POLL_MS,
    queryFn: () => adminCall<ListSeatsResponse>('ListSeats')
  });
