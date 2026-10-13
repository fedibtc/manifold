import type { GuardianFeesResponse } from '@operator-ui/types';
import { useQueries } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import {
  type BackoffPolicy,
  FEES_BACKOFF_CEILING_MS,
  FEES_POLL_MS,
  type PollFailureState,
  pollIntervalMs
} from '@/shared/api/pollingIntervals';
import { seatFanOut } from '@/shared/api/requestLimit';

export const guardianFeesKey = (seatId: string) => ['guardian-fees', seatId] as const;

const FEES_BACKOFF: BackoffPolicy = {
  baseMs: FEES_POLL_MS,
  healthyMs: FEES_POLL_MS,
  ceilingMs: FEES_BACKOFF_CEILING_MS
};

// Guardian-fee revenue is per seat and there is no aggregate verb, so the
// earnings rollup is a bounded N+1 over the fleet's seats. A seat with no
// federation yet answers with an error rather than a zero — that query simply
// fails and contributes nothing, which is the honest reading: no fee account
// exists to report on, and pretending it earned zero would be a different claim.
// Polled at the fees cadence. Two screens read it — the Overview earnings rollup
// and the Payouts guardian-fee section — so it lives in shared rather than in
// either feature; react-query keys per seat, so the two never double-poll a seat.
// It stops the moment the last reader unmounts.
//
// N+1 with no aggregate verb is the shape of the problem, not the fix: the fix is
// a fleet-level fee summary, written up in operator-ui/docs/daemon-aggregate-verbs.md.
// Until it exists the fan-out is bounded on this side — at most SEAT_FAN_OUT_LIMIT
// calls in flight, jittered so the seats do not all fire on the same tick, and
// backing off per seat while that seat keeps failing. Nothing is dropped: a seat
// past the bound is fetched a moment later, so no total is quietly narrowed, and
// a seat that cannot be read at all is still counted in unreadableFeeSeatCount.
//
// `limit: null` bounds `remittances` only, and the daemon resolves it to its own
// default. That list is recent activity for display; no money total is taken
// from it. The lifetime figure is `lifetime_remitted_msat`, a scalar the daemon
// computes over the seat's full account history.
export const useGuardianFees = (seatIds: string[]) =>
  useQueries({
    queries: seatIds.map((seatId) => ({
      queryKey: guardianFeesKey(seatId),
      refetchInterval: (query: { state: PollFailureState }) =>
        pollIntervalMs(query.state, FEES_BACKOFF, `guardian-fees:${seatId}`),
      queryFn: () =>
        seatFanOut(() =>
          adminCall<GuardianFeesResponse>({ GuardianFees: { seat_id: seatId, limit: null } })
        ),
      retry: false
    }))
  });
