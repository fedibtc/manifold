import type { GetSetupStateResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { POLL_SETUP_MS } from '@/shared/api/pollingIntervals';

export const SETUP_STATE_KEY = ['setup-state'] as const;

// The setup gate's source of truth. Polled once a minute; stays fresh for 55s so a
// route change does not refetch. retry:false keeps AuthError/NetworkError immediate
// for the boot gate instead of retrying behind a spinner.
export const useSetupState = () =>
  useQuery({
    retry: false,
    staleTime: 55_000,
    refetchInterval: POLL_SETUP_MS,
    queryKey: SETUP_STATE_KEY,
    refetchOnWindowFocus: true,
    queryFn: () => adminCall<null, GetSetupStateResponse>('get_setup_state', null)
  });
