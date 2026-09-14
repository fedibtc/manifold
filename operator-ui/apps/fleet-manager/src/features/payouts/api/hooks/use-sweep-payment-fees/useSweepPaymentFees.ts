import { newIdempotencyKey } from '@operator-ui/common-ui';
import type { PayoutDestinationResponse, SweepPaymentFeesResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useRef } from 'react';
import { PAYOUT_DESTINATION_KEY } from '@/features/payouts/api/hooks/use-payout-destination/usePayoutDestination';
import { adminCall } from '@/shared/api/adminCall';
import { PAYMENT_FEDERATIONS_KEY } from '@/shared/api/hooks/use-payment-federations/usePaymentFederations';

// One setup-payment wallet, swept through a gateway the daemon selects
// (crates/fman/core/src/admin.rs:63). There is no amount and no gateway to pass:
// the sweep takes the largest economically fundable amount, because an exact
// amount can fail on mint and routing fees.
export const useSweepPaymentFees = (federationId: string) => {
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
