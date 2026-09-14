import { newIdempotencyKey } from '@operator-ui/common-ui';
import type { PayoutDestinationResponse, SweepGuardianFeesResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useRef } from 'react';
import { PAYOUT_DESTINATION_KEY } from '@/features/payouts/api/hooks/use-payout-destination/usePayoutDestination';
import { adminCall } from '@/shared/api/adminCall';
import { guardianFeesKey } from '@/shared/api/hooks/use-guardian-fees/useGuardianFees';

// Step two of two: the ecash a collection already moved out of the pool, sent to
// the payout destination through an automatically selected gateway
// (crates/fman/core/src/admin.rs:104). Nothing still in the pool leaves this way.
export const useSweepGuardianFees = (seatId: string) => {
  const queryClient = useQueryClient();
  // A failed sweep retries under its request id, because a lost response may hide a
  // started payment. The daemon keeps the id's first destination
  // (crates/fman/specs/SPEC-admin-socket.md), so a new destination takes a new id.
  const pendingRequest = useRef<{ destination: string | null; id: string } | null>(null);

  return useMutation({
    mutationFn: () => {
      const destination =
        queryClient.getQueryData<PayoutDestinationResponse>(PAYOUT_DESTINATION_KEY)?.destination ??
        null;
      if (pendingRequest.current?.destination !== destination) {
        pendingRequest.current = { destination, id: newIdempotencyKey() };
      }
      return adminCall<SweepGuardianFeesResponse>({
        SweepGuardianFees: { seat_id: seatId, request_id: pendingRequest.current.id }
      });
    },
    onSuccess: () => {
      pendingRequest.current = null;
      void queryClient.invalidateQueries({ queryKey: guardianFeesKey(seatId) });
    }
  });
};
