import type {
  WithdrawAdvertisementRequest,
  WithdrawAdvertisementResponse
} from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { ADVERTISEMENT_KEY } from '@/shared/api/queryKeys';

export const useWithdrawAdvertisement = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (reason: string | null = null) =>
      adminCall<WithdrawAdvertisementRequest, WithdrawAdvertisementResponse>(
        'withdraw_advertisement',
        {
          reason
        }
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ADVERTISEMENT_KEY })
  });
};
