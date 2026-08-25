import type { PayoutDestinationResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { LIST_POLL_MS } from '@/shared/api/pollingIntervals';

export const PAYOUT_DESTINATION_KEY = ['payout-destination'] as const;

// The one Lightning destination every revenue sweep leaves through
// (crates/fman/core/src/fleet.rs::payout_destination). `null` is a fact — no
// destination is configured — and every sweep verb refuses while it holds.
export const usePayoutDestination = () =>
  useQuery({
    queryKey: PAYOUT_DESTINATION_KEY,
    refetchInterval: LIST_POLL_MS,
    queryFn: () => adminCall<PayoutDestinationResponse>('PayoutDestination')
  });
