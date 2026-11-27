import type { PayoutDestinationResponse, SweepGuardianFeesResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useRef } from 'react';
import { PAYOUT_DESTINATION_KEY } from '@/features/payouts/api/hooks/use-payout-destination/usePayoutDestination';
import { PAYOUTS_KEY } from '@/features/payouts/api/hooks/use-payouts/usePayouts';
import {
  type PayoutMethodName,
  type SweepRequest,
  sweepRequestFor
} from '@/features/payouts/utils/sweepRequest';
import { adminCall } from '@/shared/api/adminCall';
import { guardianFeesKey } from '@/shared/api/hooks/use-guardian-fees/useGuardianFees';

// Step two of two: the ecash a collection already moved out of the pool, paid
// out by `method`: to the payout destination through an automatically selected
// gateway, or as one ecash token (crates/mg/core/src/admin.rs). Nothing still
// in the pool leaves this way.
export const useSweepGuardianFees = (seatId: string, method: PayoutMethodName) => {
  const queryClient = useQueryClient();
  const pendingRequest = useRef<SweepRequest | null>(null);

  return useMutation({
    mutationFn: () => {
      const destination =
        queryClient.getQueryData<PayoutDestinationResponse>(PAYOUT_DESTINATION_KEY)?.destination;
      pendingRequest.current = sweepRequestFor(pendingRequest.current, method, destination);
      return adminCall<SweepGuardianFeesResponse>({
        SweepGuardianFees: { seat_id: seatId, request_id: pendingRequest.current.id, method }
      });
    },
    onSuccess: () => {
      pendingRequest.current = null;
      void queryClient.invalidateQueries({ queryKey: PAYOUTS_KEY });
      void queryClient.invalidateQueries({ queryKey: guardianFeesKey(seatId) });
    }
  });
};
