import type { ShowCapacityResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';

export const CAPACITY_KEY = ['capacity'] as const;

/** The durable seat ceiling and how many slots are still open under it. The
 *  ceiling is what the operator edits; the free slots are what makes the
 *  current value readable to them. */
export const useCapacity = () =>
  useQuery({
    queryKey: CAPACITY_KEY,
    queryFn: () => adminCall<ShowCapacityResponse>('ShowCapacity')
  });
