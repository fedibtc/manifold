import type { ResolveManualReviewRequest, ResolveManualReviewResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { WALLET_OPERATIONS_KEY } from '@/features/funds/api/hooks/use-funds/useFunds';
import { walletOperationKey } from '@/features/funds/api/hooks/use-wallet-operation/useWalletOperation';
import { adminCall } from '@/shared/api/adminCall';

// The only exit from manual review. Sync skips a frozen operation and retry
// refuses it, so nothing else moves it and no amount of waiting will.
export const useResolveManualReview = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: ResolveManualReviewRequest) =>
      adminCall<ResolveManualReviewRequest, ResolveManualReviewResponse>(
        'resolve_manual_review',
        request
      ),
    onSuccess: (_response, request) => {
      queryClient.invalidateQueries({ queryKey: WALLET_OPERATIONS_KEY });
      queryClient.invalidateQueries({ queryKey: walletOperationKey(request.operation_id) });
    }
  });
};
