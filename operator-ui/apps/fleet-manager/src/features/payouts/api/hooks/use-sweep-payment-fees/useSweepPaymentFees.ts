import type { PayoutDestinationResponse, SweepPaymentFeesResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useRef } from 'react';
import { PAYOUT_DESTINATION_KEY } from '@/features/payouts/api/hooks/use-payout-destination/usePayoutDestination';
import { type SweepRequest, sweepRequestFor } from '@/features/payouts/utils/sweepRequest';
import { adminCall } from '@/shared/api/adminCall';
import { PAYMENT_FEDERATIONS_KEY } from '@/shared/api/hooks/use-payment-federations/usePaymentFederations';

// One setup-payment wallet, swept through a gateway the daemon selects
// (crates/fman/core/src/admin.rs:63). There is no amount and no gateway to pass:
// the sweep takes the largest economically fundable amount, because an exact
// amount can fail on mint and routing fees.
export const useSweepPaymentFees = (federationId: string) => {
  const queryClient = useQueryClient();
  const pendingRequest = useRef<SweepRequest | null>(null);

  return useMutation({
    mutationFn: () => {
      const destination =
        queryClient.getQueryData<PayoutDestinationResponse>(PAYOUT_DESTINATION_KEY)?.destination;
      pendingRequest.current = sweepRequestFor(pendingRequest.current, destination);
      return adminCall<SweepPaymentFeesResponse>({
        SweepPaymentFees: { federation_id: federationId, request_id: pendingRequest.current.id }
      });
    },
    onSuccess: () => {
      pendingRequest.current = null;
      void queryClient.invalidateQueries({ queryKey: PAYMENT_FEDERATIONS_KEY });
    }
  });
};
