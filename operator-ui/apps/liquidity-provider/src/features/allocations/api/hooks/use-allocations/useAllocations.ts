import type { ListAllocationsRequest, ListAllocationsResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { POLL_ACTIVE_MS } from '@/shared/api/pollingIntervals';
import { hasNonTerminalStatus } from '@/shared/utils/allocationPolling';
import { summaryStatus } from '@/shared/utils/allocationStatus';

export const ALLOCATIONS_KEY = ['allocations'] as const;

const DEFAULT_LIMIT = 50;

const hasNonTerminalAllocation = (data: ListAllocationsResponse | undefined): boolean =>
  hasNonTerminalStatus(data?.allocations.items.map(summaryStatus) ?? []);

// list_allocations. Params default to a first page; the resolved request is
// part of the query key so param changes refetch cleanly. Polls at the active
// cadence while any row is still non-terminal, matching the allocation detail
// hook.
export const useAllocations = (params?: Partial<ListAllocationsRequest>) => {
  const request: ListAllocationsRequest = {
    page: params?.page ?? { cursor: null, limit: DEFAULT_LIMIT },
    time_range: params?.time_range ?? null
  };
  return useQuery({
    retry: false,
    queryKey: [...ALLOCATIONS_KEY, request],
    refetchInterval: (query) =>
      hasNonTerminalAllocation(query.state.data) ? POLL_ACTIVE_MS : false,
    queryFn: () =>
      adminCall<ListAllocationsRequest, ListAllocationsResponse>('list_allocations', request)
  });
};
