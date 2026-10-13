import { useQuery } from '@tanstack/react-query';
import { seatStatusQueryOptions } from '@/features/seats/api/hooks/use-seat-status/seatStatusQuery';

// The seat detail page watches one seat: its formation while it is forming, its
// health for as long as the page is open. Both cadences, the cache key and the
// backoff are the list's — see seatStatusQuery.ts — because two screens showing
// one seat that disagree about it are worse than either answer alone.
export const useSeatStatus = (seatId: string) => useQuery(seatStatusQueryOptions(seatId));
