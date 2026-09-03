import { newIdempotencyKey } from '@operator-ui/common-ui';
import type { SweepGuardianFeesResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useRef } from 'react';
import { adminCall } from '@/shared/api/adminCall';
import { guardianFeesKey } from '@/shared/api/hooks/use-guardian-fees/useGuardianFees';

// Step two of two: the ecash a collection already moved out of the pool, sent to
// the payout destination through an automatically selected gateway
// (crates/fman/core/src/admin.rs:104). Nothing still in the pool leaves this way.
export const useSweepGuardianFees = (seatId: string) => {
  const queryClient = useQueryClient();
  const requestId = useRef(newIdempotencyKey());

  return useMutation({
    mutationFn: () =>
      adminCall<SweepGuardianFeesResponse>({
        SweepGuardianFees: { seat_id: seatId, request_id: requestId.current }
      }),
    onSuccess: () => {
      requestId.current = newIdempotencyKey();
      void queryClient.invalidateQueries({ queryKey: guardianFeesKey(seatId) });
    }
  });
};
