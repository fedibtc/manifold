// Central polling policy (fman-dashboard.md "Live data freshness"). All hooks
// that poll must pull their interval from here instead of hardcoding
// milliseconds. useSeatReports and useGuardianFees fan out one call per seat,
// so their cadence is a cost multiplier (interval × seat count) — argue
// changes to these constants in seats, not milliseconds.
export const SEAT_FORMATION_POLL_MS = 5_000; // seat status, only while non-terminal
export const LIST_POLL_MS = 30_000; // ListSeats, ListPaymentFederations
export const FEES_POLL_MS = 60_000; // GuardianFees, screens that display fees only

// Ceilings the backoff below decays toward. A poll that keeps failing costs the
// same as one that succeeds, so a failing one must get cheaper over the lifetime
// of the tab rather than repeat its healthy cadence forever.
export const SEAT_BACKOFF_CEILING_MS = 60_000;
export const FEES_BACKOFF_CEILING_MS = 600_000;

const JITTER_RATIO = 0.2;
// 2 ** 20 × any base here is already past every ceiling; the clamp only keeps
// the arithmetic finite for a tab left open on a dead daemon for weeks.
const MAX_DOUBLINGS = 20;

// One offset per poller, drawn once and kept. Drawn: two dashboards restarted
// together, or forty seats on one screen, would otherwise fire on the same tick
// forever. Kept: react-query restarts a poll timer whenever the interval it
// computes differs from the running one, and it recomputes on every render — an
// offset redrawn per call would reset the timer on every render, and a screen
// that renders more often than it polls would then never poll at all.
const offsets = new Map<string, number>();

const offsetFor = (seed: string): number => {
  const kept = offsets.get(seed);
  if (kept !== undefined) return kept;

  const drawn = Math.random() * 2 - 1;
  offsets.set(seed, drawn);
  return drawn;
};

/** ±20% of `ms`, fixed for the life of the tab for a given `seed`. */
export const withJitter = (ms: number, seed: string): number =>
  Math.round(ms * (1 + JITTER_RATIO * offsetFor(seed)));

/** The part of a react-query state this policy reads. */
export interface PollFailureState {
  /**
   * react-query's own query status. Read instead of `error` because a query with
   * no data has its error CLEARED the moment the next fetch starts: mid-flight it
   * looks exactly like a query that has never failed, and a streak that trusted
   * that would reset itself on every retry and never back off at all.
   */
  status: 'pending' | 'error' | 'success';
  errorUpdateCount: number;
}

export interface BackoffPolicy {
  /** The delay the first failure waits, and the unit the backoff doubles in. */
  baseMs: number;
  /** The cadence this query polls at while healthy — the gap that is not failure. */
  healthyMs: number;
  ceilingMs: number;
}

// react-query exposes no consecutive-failure counter, so the streak is kept here,
// beside the jitter offsets, as the other half of one poller's state.
// `fetchFailureCount` counts retries WITHIN one fetch and is reset when the next
// fetch starts, so under `retry: false` it never exceeds 1; `errorUpdateCount`
// counts every error the query has ever taken and no recovery resets it. A streak
// has to be reset by the success that ends it, which is what neither of those does
// and this map is here to do.
//
// It advances on a change in `errorUpdateCount`, never on being called: react-query
// recomputes the interval on every render, so a streak that counted calls would
// grow with rendering rather than with failing. Evaluating one query state twice
// returns the same answer, which is also what keeps the poll timer from resetting.
interface FailureStreak {
  /** The query's lifetime error count when this streak last advanced. */
  errorUpdateCount: number;
  consecutive: number;
}

const streaks = new Map<string, FailureStreak>();

// No entry yet is the same as a poller that has never failed, and -1 is below the
// first error count a query can report, so its first failure counts as a first.
const NEVER_FAILED: FailureStreak = { errorUpdateCount: -1, consecutive: 0 };

const failureStreak = (state: PollFailureState, seed: string): number => {
  const seen = streaks.get(seed) ?? NEVER_FAILED;

  // A success ends the streak, whatever it had grown to: the next failure is a
  // first failure again and gets the prompt retry a first failure is owed.
  if (state.status === 'success') {
    streaks.set(seed, { errorUpdateCount: state.errorUpdateCount, consecutive: 0 });
    return 0;
  }

  // Anything else — a fetch in flight, or the failure that is already counted —
  // leaves the streak where it is, which is also what keeps the interval stable
  // and the poll timer from being torn down and rebuilt.
  if (state.status !== 'error' || state.errorUpdateCount === seen.errorUpdateCount) {
    return seen.consecutive;
  }

  // The count only ever grows within one query, so a smaller one belongs to a
  // different query under the same seed — a remount, or a fresh cache — whose
  // streak starts at its own first failure rather than inheriting the last one's.
  const consecutive = state.errorUpdateCount > seen.errorUpdateCount ? seen.consecutive + 1 : 1;

  streaks.set(seed, { errorUpdateCount: state.errorUpdateCount, consecutive });
  return consecutive;
};

/**
 * The healthy cadence while the last poll succeeded; on failure `baseMs` for the
 * first, doubling per consecutive failure, capped at `ceilingMs`. Jittered either
 * way, so instances restarted together do not stay in lockstep. Window focus and
 * every explicit Retry control stay untouched, so an operator can always force an
 * immediate attempt no matter how far the automatic cadence has decayed.
 */
export const pollIntervalMs = (
  state: PollFailureState,
  policy: BackoffPolicy,
  seed: string
): number => {
  const streak = failureStreak(state, seed);
  if (streak === 0) return withJitter(policy.healthyMs, seed);

  const steps = Math.min(streak - 1, MAX_DOUBLINGS);
  return withJitter(Math.min(policy.baseMs * 2 ** steps, policy.ceilingMs), seed);
};
