import type { SetPayoutDestinationResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { PAYOUT_DESTINATION_KEY } from '@/features/payouts/api/hooks/use-payout-destination/usePayoutDestination';
import { adminCall } from '@/shared/api/adminCall';

// SetPayoutDestination answers with the stored view, in the same shape
// PayoutDestination reads it back (crates/fman/core/src/admin.rs:342-344), so the
// answer seeds the cache directly — a write needs no follow-up read, and an
// invalidation here would blank the destination for one render before it returned.
export const useSetPayoutDestination = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (destination: string | null) =>
      adminCall<SetPayoutDestinationResponse>({ SetPayoutDestination: { destination } }),
    onSuccess: (stored) => {
      queryClient.setQueryData(PAYOUT_DESTINATION_KEY, stored);
    }
  });
};
