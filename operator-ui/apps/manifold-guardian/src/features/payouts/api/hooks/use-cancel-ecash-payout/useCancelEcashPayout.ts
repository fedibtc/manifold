import type { CancelEcashPayoutResponse, PayoutScope } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { settlePayout } from '@/features/payouts/api/hooks/use-payout-status/usePayoutStatus';
import { PAYOUTS_KEY } from '@/features/payouts/api/hooks/use-payouts/usePayouts';
import { balanceKeyFor } from '@/features/payouts/utils/balanceKey';
import { sourceOfScope } from '@/features/payouts/utils/payoutSource';
import { adminCall } from '@/shared/api/adminCall';

// Takes a token back into the wallet it left, also after the operator marked it
// redeemed. The daemon answers once the federation has settled the race with
// the operator's own redemption, as `reclaimed` or `already_spent`. The
// mutation takes the payout's scope, to refresh the balance it refills.
export const useCancelEcashPayout = (requestId: string) => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (_scope: PayoutScope) =>
      adminCall<CancelEcashPayoutResponse>({ CancelEcashPayout: { request_id: requestId } }),
    onSuccess: (payout) => settlePayout(queryClient, payout),
    onSettled: (_payout, _error, scope) => {
      void queryClient.invalidateQueries({ queryKey: PAYOUTS_KEY });
      void queryClient.invalidateQueries({ queryKey: balanceKeyFor(sourceOfScope(scope)) });
    }
  });
};
