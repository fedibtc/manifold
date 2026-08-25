import type { GetFundsResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { POLL_STANDARD_MS } from '@/shared/api/pollingIntervals';

export const FUNDS_KEY = ['funds'] as const;
export const WALLET_OPERATIONS_KEY = ['wallet-operations'] as const;

// Funds + inventory snapshot for the Funds screen. Mirrors useSetupState:
// retry:false surfaces AuthError/NetworkError immediately instead of behind a
// spinner; polled per the standard cadence, fresh for 55s so a route change
// does not refetch.
export const useFunds = () =>
  useQuery({
    retry: false,
    staleTime: 55_000,
    refetchInterval: POLL_STANDARD_MS,
    queryKey: FUNDS_KEY,
    refetchOnWindowFocus: true,
    queryFn: () => adminCall<null, GetFundsResponse>('get_funds', null)
  });
