import type { RetryFundingStepRequest, RetryFundingStepResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { ALLOCATION_KEY } from '@/features/allocations/api/hooks/use-allocation/useAllocation';
import { ALLOCATIONS_KEY } from '@/features/allocations/api/hooks/use-allocations/useAllocations';
import { adminCall } from '@/shared/api/adminCall';

// retry_funding_step. Re-attempts a single failed wallet-operation step; on
// success both the list and the detail are invalidated so the row status and
// timeline reflect the daemon's new pending state.
export const useRetryFundingStep = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: RetryFundingStepRequest) =>
      adminCall<RetryFundingStepRequest, RetryFundingStepResponse>('retry_funding_step', request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ALLOCATIONS_KEY });
      queryClient.invalidateQueries({ queryKey: ALLOCATION_KEY });
    }
  });
};
