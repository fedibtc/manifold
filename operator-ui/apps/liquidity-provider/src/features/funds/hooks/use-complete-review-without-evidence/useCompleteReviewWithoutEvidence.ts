import type {
  CompleteReviewWithoutEvidenceRequest,
  CompleteReviewWithoutEvidenceResponse
} from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { WALLET_OPERATIONS_KEY } from '@/features/funds/api/hooks/use-funds/useFunds';
import { walletOperationKey } from '@/features/funds/api/hooks/use-wallet-operation/useWalletOperation';
import { adminCall } from '@/shared/api/adminCall';

// complete_review_without_evidence. The exit from manual review for a send
// whose outcome the operator established off chain: `resolve_manual_review`
// refuses a `completed` resolution it cannot back with chain evidence, and
// this verb completes on the operator's assertion instead.
//
// Kept as its own hook rather than a flag on useResolveManualReview, for the
// reason the daemon splits the two verbs: an unverified completion must not
// be reachable through the call that looks verified.
export const useCompleteReviewWithoutEvidence = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: CompleteReviewWithoutEvidenceRequest) =>
      adminCall<CompleteReviewWithoutEvidenceRequest, CompleteReviewWithoutEvidenceResponse>(
        'complete_review_without_evidence',
        request
      ),
    onSuccess: (_response, request) => {
      queryClient.invalidateQueries({ queryKey: WALLET_OPERATIONS_KEY });
      queryClient.invalidateQueries({ queryKey: walletOperationKey(request.operation_id) });
    }
  });
};
