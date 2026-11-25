import type { ListPayoutsResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { movesOnItsOwn } from '@/features/payouts/utils/payoutStage';
import { adminCall } from '@/shared/api/adminCall';
import { LIST_POLL_MS, PAYOUT_PROGRESS_POLL_MS } from '@/shared/api/pollingIntervals';

export const PAYOUTS_KEY = ['payouts'] as const;

const isMoving = (response: ListPayoutsResponse | undefined) =>
  response?.payouts.some(movesOnItsOwn) ?? false;

// Payouts of both methods, newest first, without tokens: the In progress card
// and History read the same list. An older token still waiting to be redeemed
// is always in it.
export const usePayouts = () =>
  useQuery({
    queryKey: PAYOUTS_KEY,
    refetchInterval: (query) =>
      isMoving(query.state.data) ? PAYOUT_PROGRESS_POLL_MS : LIST_POLL_MS,
    queryFn: () => adminCall<ListPayoutsResponse>('ListPayouts')
  });
