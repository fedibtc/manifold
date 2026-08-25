import type { SeatStatusResponse } from '@operator-ui/types';
import { adminCall } from '@/shared/api/adminCall';
import {
  type BackoffPolicy,
  LIST_POLL_MS,
  type PollFailureState,
  pollIntervalMs,
  SEAT_BACKOFF_CEILING_MS,
  SEAT_FORMATION_POLL_MS
} from '@/shared/api/pollingIntervals';
import { seatFanOut } from '@/shared/api/requestLimit';
import { isSeatForming, isSeatReportFinal } from '@/shared/utils/seatStatus';

export const seatStatusKey = (seatId: string) => ['seat-status', seatId] as const;

// A seat still forming is one an operator is watching right now, so it is asked
// often — the cadence the whole DKG wait is paced by.
const FORMING_POLICY: BackoffPolicy = {
  baseMs: SEAT_FORMATION_POLL_MS,
  healthyMs: SEAT_FORMATION_POLL_MS,
  ceilingMs: SEAT_BACKOFF_CEILING_MS
};

// A settled seat is watched by nobody but this screen, and its health is exactly
// what changes without anyone doing anything. It slows to the fleet-wide list
// cadence — one call per seat per 30 s is the same order as ListSeats itself —
// rather than stopping, which is what left a guardian that died after formation
// showing its last fetched health until someone happened to refocus the tab.
const SETTLED_POLICY: BackoffPolicy = {
  baseMs: SEAT_FORMATION_POLL_MS,
  healthyMs: LIST_POLL_MS,
  ceilingMs: SEAT_BACKOFF_CEILING_MS
};

// The part of a react-query `Query` this policy reads. Structural, so both
// `useQuery` and `useQueries` hand their own `Query` straight to it.
interface SeatStatusQuery {
  queryKey: readonly unknown[];
  state: PollFailureState & { data?: SeatStatusResponse };
}

// One seed per seat, shared by every screen watching that seat, so the jitter
// offset and the failure streak belong to the seat rather than to whichever
// screen happens to be open.
const seedFor = (queryKey: readonly unknown[]): string => queryKey.join(':');

/**
 * The single polling policy for one seat's status, whatever screen asked.
 *
 * Only a decommissioned report ends polling. A report that never arrived and a
 * report that has finished forming both keep their timer — the first because a
 * row with nothing in it must keep trying, the second because health outlives
 * formation.
 */
export const seatStatusRefetchInterval = (query: SeatStatusQuery): number | false => {
  const report = query.state.data?.report;
  if (report !== undefined && isSeatReportFinal(report)) return false;

  const policy = report === undefined || isSeatForming(report) ? FORMING_POLICY : SETTLED_POLICY;
  return pollIntervalMs(query.state, policy, seedFor(query.queryKey));
};

export interface SeatStatusQueryConfig {
  /**
   * Route the call through the shared per-seat fan-out budget. The list sets it:
   * a whole fleet's worth of background reads has to be spread over a few round
   * trips. The detail page does not: one foreground read has no fleet to spread,
   * and queueing it behind the list's would delay the screen being looked at.
   */
  fanOut?: boolean;
}

/**
 * The one set of options behind one `seat-status` cache key.
 *
 * The list cannot call `useSeatStatus` per seat — a hook cannot be called from a
 * loop over a list whose length changes — so it maps these options through
 * `useQueries` instead. Key and polling policy are the same object either way;
 * `fanOut` is the only thing a caller may vary.
 */
export const seatStatusQueryOptions = (seatId: string, { fanOut }: SeatStatusQueryConfig = {}) => ({
  queryKey: seatStatusKey(seatId),
  refetchInterval: seatStatusRefetchInterval,
  queryFn: () => {
    const call = () => adminCall<SeatStatusResponse>({ SeatStatus: { seat_id: seatId } });
    return fanOut ? seatFanOut(call) : call();
  }
});
