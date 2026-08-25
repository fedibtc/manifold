import type { CancelAllocationRequest, CancelAllocationResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { ALLOCATION_KEY } from '@/features/allocations/api/hooks/use-allocation/useAllocation';
import { ALLOCATIONS_KEY } from '@/features/allocations/api/hooks/use-allocations/useAllocations';
import { adminCall } from '@/shared/api/adminCall';

// cancel_allocation. Stops a non-terminal allocation; on success both the
// list and the detail are invalidated so the row status and timeline reflect
// the cancellation.
export const useCancelAllocation = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: CancelAllocationRequest) =>
      adminCall<CancelAllocationRequest, CancelAllocationResponse>('cancel_allocation', request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ALLOCATIONS_KEY });
      queryClient.invalidateQueries({ queryKey: ALLOCATION_KEY });
    }
  });
};
