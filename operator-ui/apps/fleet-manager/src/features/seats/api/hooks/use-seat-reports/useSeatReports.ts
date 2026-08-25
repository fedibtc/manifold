import { useQueries } from '@tanstack/react-query';
import { seatStatusQueryOptions } from '@/features/seats/api/hooks/use-seat-status/seatStatusQuery';

// The seats list has no aggregate verb for per-seat phase/health — only SeatStatus,
// one seat at a time. Fleets are small (a bounded seat capacity per operator), so
// fetching each active seat's status in parallel is a deliberate, bounded N+1 —
// unlike Overview, which skips this because it only needs a boolean rollup.
// Decommissioned seats are excluded by the caller: their list row never shows a
// health chip, so there's nothing worth fetching for them.
//
// "Small" is the operator's word, not a bound this file can enforce, so the
// fan-out is bounded: at most SEAT_FAN_OUT_LIMIT calls in flight, jittered so a
// whole fleet does not fire on one tick, and backing off per seat while that
// seat's last poll keeps failing. A seat over the bound waits its turn instead of
// being skipped, so no row silently stops reporting. The real fix is a
// fleet-level status verb — see operator-ui/docs/daemon-aggregate-verbs.md.
//
// Every other decision — the key, the cadences, when polling ends — belongs to
// seatStatusQueryOptions, which the seat detail page reads through as well.
export const useSeatReports = (seatIds: string[]) =>
  useQueries({
    queries: seatIds.map((seatId) => seatStatusQueryOptions(seatId, { fanOut: true }))
  });
