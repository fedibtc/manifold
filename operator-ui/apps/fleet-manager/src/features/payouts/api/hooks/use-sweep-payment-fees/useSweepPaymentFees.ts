import { newIdempotencyKey } from '@operator-ui/common-ui';
import type { SweepPaymentFeesResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useRef } from 'react';
import { adminCall } from '@/shared/api/adminCall';
import { PAYMENT_FEDERATIONS_KEY } from '@/shared/api/hooks/use-payment-federations/usePaymentFederations';

// One setup-payment wallet, swept through a gateway the daemon selects
// (crates/fman/core/src/admin.rs:63). There is no amount and no gateway to pass:
// the sweep takes the largest economically fundable amount, because an exact
// amount can fail on mint and routing fees.
export const useSweepPaymentFees = (federationId: string) => {
  const queryClient = useQueryClient();
  const requestId = useRef(newIdempotencyKey());

  return useMutation({
    mutationFn: () =>
      adminCall<SweepPaymentFeesResponse>({
        SweepPaymentFees: { federation_id: federationId, request_id: requestId.current }
      }),
    onSuccess: () => {
      requestId.current = newIdempotencyKey();
      void queryClient.invalidateQueries({ queryKey: PAYMENT_FEDERATIONS_KEY });
    }
  });
};
