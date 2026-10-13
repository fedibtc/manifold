import type { ListPaymentFederationsResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { LIST_POLL_MS } from '@/shared/api/pollingIntervals';

export const PAYMENT_FEDERATIONS_KEY = ['payment-federations'] as const;

export const usePaymentFederations = () =>
  useQuery({
    queryKey: PAYMENT_FEDERATIONS_KEY,
    refetchInterval: LIST_POLL_MS,
    queryFn: () => adminCall<ListPaymentFederationsResponse>('ListPaymentFederations')
  });
