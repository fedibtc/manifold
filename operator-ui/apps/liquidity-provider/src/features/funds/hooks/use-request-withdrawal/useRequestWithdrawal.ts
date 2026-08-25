import type { RequestWithdrawalRequest, RequestWithdrawalResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { FUNDS_KEY, WALLET_OPERATIONS_KEY } from '@/features/funds/api/hooks/use-funds/useFunds';
import { adminCall } from '@/shared/api/adminCall';

// Request an on-chain withdrawal. On success available_balance/pending_outgoing
// and the wallet-operations list both change, so invalidate both.
export const useRequestWithdrawal = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: RequestWithdrawalRequest) =>
      adminCall<RequestWithdrawalRequest, RequestWithdrawalResponse>('request_withdrawal', request),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: FUNDS_KEY });
      void queryClient.invalidateQueries({ queryKey: WALLET_OPERATIONS_KEY });
    }
  });
};
