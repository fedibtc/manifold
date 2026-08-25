import type { GetAdminAllocationRequest, GetAdminAllocationResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { POLL_ACTIVE_MS } from '@/shared/api/pollingIntervals';
import { hasNonTerminalStatus } from '@/shared/utils/allocationPolling';
import { detailStatus } from '@/shared/utils/allocationStatus';

export const ALLOCATION_KEY = ['allocation'] as const;

const isAllocationNonTerminal = (data: GetAdminAllocationResponse | undefined): boolean =>
  data ? hasNonTerminalStatus([detailStatus(data.allocation)]) : false;

// get_allocation detail. Disabled until a federation is selected. Polls at the
// active cadence while the allocation is still non-terminal.
export const useAllocation = (federationId: string | null) =>
  useQuery({
    enabled: !!federationId,
    queryKey: [...ALLOCATION_KEY, federationId],
    refetchInterval: (query) =>
      isAllocationNonTerminal(query.state.data) ? POLL_ACTIVE_MS : false,
    queryFn: () =>
      adminCall<GetAdminAllocationRequest, GetAdminAllocationResponse>('get_allocation', {
        federation_id: federationId as string
      })
  });
