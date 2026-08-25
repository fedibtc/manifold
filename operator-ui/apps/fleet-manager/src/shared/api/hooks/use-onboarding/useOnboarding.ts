import type { OnboardingResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import {
  type BackoffPolicy,
  type PollFailureState,
  pollIntervalMs
} from '@/shared/api/pollingIntervals';

export const ONBOARDING_KEY = ['onboarding'] as const;

// The same fetch the hook runs, exported so an imperative `fetchQuery` writes to
// this key through one definition. `refetchQueries` cannot stand in for it: it
// refetches queries that already exist, and the callers below run at the moment
// the identity changes, when the cached answer may have just been dropped.
export const fetchOnboarding = () => adminCall<OnboardingResponse>('Onboarding');

// Doubles as the boot gate's smoke test (see useBootStatus): FM has no unauthenticated
// liveness verb, so Onboarding — the lightest real admin call — is what tells the boot
// gate whether the daemon is reachable and whether the session is authenticated.
// retry:false keeps AuthError/NetworkError immediate instead of retrying behind a spinner.
//
// A failed call retries far faster than a healthy one polls. Both blocking states
// can be resolved by the daemon alone — a trusted-proxy listener stops requiring
// the session, an unreachable daemon comes back — and neither gives the operator
// anything to click, so a minute-long wait behind a sign-in prompt reads as the
// dashboard having given up.
//
// That argument holds for the first failures, not for the lifetime of the tab: a
// fixed 5s retry in front of a daemon that stays down is 720 calls an hour, in
// lockstep across every instance restarted together. So the retry starts prompt
// and decays to the healthy cadence, jittered. Nothing is given up by decaying —
// refetchOnWindowFocus and the Retry button on the boot screen both force an
// immediate attempt whenever the operator has reason to think it will work now.
const HEALTHY_POLL_MS = 60_000;
const BLOCKED_BASE_POLL_MS = 5_000;

const BLOCKED_BACKOFF: BackoffPolicy = {
  baseMs: BLOCKED_BASE_POLL_MS,
  healthyMs: HEALTHY_POLL_MS,
  ceilingMs: HEALTHY_POLL_MS
};

export const onboardingPollMs = (state: PollFailureState): number =>
  pollIntervalMs(state, BLOCKED_BACKOFF, 'onboarding');

// refetchOnMount:false is load-bearing, not a tuning choice. This query decides
// whether the gates below it render, and those gated components observe it too
// (SetupGate, and the setup wizard's authorization watch). With the default, a
// gate opening would mount a second observer, whose refetch resets this query to
// `pending`, which closes the gate, which unmounts that observer — an unbounded
// mount/unmount flap that stops only when the browser's connection pool jams.
// Freshness comes from the interval and window focus instead, which no amount of
// mounting can retrigger.
export const useOnboarding = () =>
  useQuery({
    retry: false,
    staleTime: 55_000,
    refetchInterval: (query: { state: PollFailureState & { data?: OnboardingResponse } }) =>
      query.state.data?.runtime === 'starting' ? 1_000 : onboardingPollMs(query.state),
    refetchOnMount: false,
    queryKey: ONBOARDING_KEY,
    refetchOnWindowFocus: true,
    queryFn: fetchOnboarding
  });
