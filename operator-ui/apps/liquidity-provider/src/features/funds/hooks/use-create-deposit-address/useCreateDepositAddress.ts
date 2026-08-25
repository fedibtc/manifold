import type { CreateDepositAddressRequest, CreateDepositAddressResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { FUNDS_KEY, WALLET_OPERATIONS_KEY } from '@/features/funds/api/hooks/use-funds/useFunds';
import { adminCall } from '@/shared/api/adminCall';

// Create a fresh top-up address. On success the funds snapshot (pending_incoming)
// and the wallet-operations list (new deposit op) both change, so invalidate both.
export const useCreateDepositAddress = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () =>
      adminCall<CreateDepositAddressRequest, CreateDepositAddressResponse>(
        'create_deposit_address',
        { label: null }
      ),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: FUNDS_KEY });
      void queryClient.invalidateQueries({ queryKey: WALLET_OPERATIONS_KEY });
    }
  });
};
