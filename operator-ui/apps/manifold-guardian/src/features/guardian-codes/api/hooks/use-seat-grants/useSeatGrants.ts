import type { ListSeatGrantsResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';

export const SEAT_GRANTS_KEY = ['seat-grants'] as const;

// The daemon lists oldest first; the operator looks for the code they just made.
const newestFirst = ({ grants }: ListSeatGrantsResponse): ListSeatGrantsResponse => ({
  grants: [...grants].reverse()
});

/** The guardian codes this host issued, newest first (SPEC-seat-grant). */
export const useSeatGrants = (enabled = true) =>
  useQuery({
    enabled,
    queryKey: SEAT_GRANTS_KEY,
    queryFn: () => adminCall<ListSeatGrantsResponse>('ListSeatGrants'),
    select: newestFirst
  });
