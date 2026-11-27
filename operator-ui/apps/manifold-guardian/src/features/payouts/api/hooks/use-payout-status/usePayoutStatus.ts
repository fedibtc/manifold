import type { PayoutJobStatusResponse } from '@operator-ui/types';
import { type QueryClient, useQuery } from '@tanstack/react-query';
import { PAYOUTS_KEY } from '@/features/payouts/api/hooks/use-payouts/usePayouts';
import { movesOnItsOwn } from '@/features/payouts/utils/payoutStage';
import { adminCall } from '@/shared/api/adminCall';
import { PAYOUT_STATUS_POLL_MS } from '@/shared/api/pollingIntervals';

export const payoutStatusKey = (requestId: string) => ['payout-status', requestId] as const;

const isMoving = (payout: PayoutJobStatusResponse | undefined) =>
  payout !== undefined && movesOnItsOwn(payout);

// One payout as the daemon reads it now, with its token while it is issued.
// The dialog that shows it polls only while the payout moves on its own: a
// Lightning payment in flight, or a reclaim the federation has not settled.
export const usePayoutStatus = (requestId: string) =>
  useQuery({
    queryKey: payoutStatusKey(requestId),
    refetchInterval: (query) => (isMoving(query.state.data) ? PAYOUT_STATUS_POLL_MS : false),
    queryFn: () => adminCall<PayoutJobStatusResponse>({ PayoutStatus: { request_id: requestId } })
  });

/** Seed the open dialog with a verb's answer, and refresh the list it changed. */
export const settlePayout = (queryClient: QueryClient, payout: PayoutJobStatusResponse) => {
  queryClient.setQueryData(payoutStatusKey(payout.job.request_id), payout);
  void queryClient.invalidateQueries({ queryKey: PAYOUTS_KEY });
};
