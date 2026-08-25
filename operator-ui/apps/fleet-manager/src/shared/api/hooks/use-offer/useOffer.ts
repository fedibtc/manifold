import type { ShowPlansResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';

export const OFFER_KEY = ['offer'] as const;

// The verb is still called ShowPlans, but the plan list is only ever the stored
// price rendered as the wire states it — no price is an empty list, and a price
// of zero is one free plan.
export const useOffer = () =>
  useQuery({
    queryKey: OFFER_KEY,
    queryFn: () => adminCall<ShowPlansResponse>('ShowPlans')
  });
