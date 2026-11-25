import type { ConfirmEcashPayoutResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { payoutStatusKey } from '@/features/payouts/api/hooks/use-payout-status/usePayoutStatus';
import { PAYOUTS_KEY } from '@/features/payouts/api/hooks/use-payouts/usePayouts';
import { adminCall } from '@/shared/api/adminCall';

// The operator's word that the token was redeemed: a mark on the job, which
// closes the payout on the page. A later cancel can still take unredeemed
// notes back. The mutation settles once the payout has been read again.
export const useConfirmEcashPayout = (requestId: string) => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: () =>
      adminCall<ConfirmEcashPayoutResponse>({ ConfirmEcashPayout: { request_id: requestId } }),
    onSuccess: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: payoutStatusKey(requestId) }),
        queryClient.invalidateQueries({ queryKey: PAYOUTS_KEY })
      ])
  });
};
