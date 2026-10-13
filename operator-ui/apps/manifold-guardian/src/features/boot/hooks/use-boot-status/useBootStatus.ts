import { hashKey, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { AccessDeniedError, AuthError, isDaemonUnreachable } from '@/shared/api/errors';
import { ONBOARDING_KEY, useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';

export type BootStatus =
  | 'daemon-unreachable'
  | 'access-denied'
  | 'needs-auth'
  | 'booting'
  | 'ready';

export interface BootState {
  status: BootStatus;
  /** The failure the status was read from, carried so the screen can state what
   *  was actually observed instead of guessing at it. */
  failure: unknown;
  onRetry: () => void;
}

// Boot order: (1) unauthorized → G2; (2) refused → G3; (3) daemon unreachable → G1;
// (4) ready.
// FM has no unauthenticated liveness verb (no /health route), so Onboarding — the
// lightest real admin call — is the single smoke test for daemon reachability,
// session auth, and permission. An unreachable-class failure (see isDaemonUnreachable:
// no transport, a 5xx, or an answer that is not the admin protocol) only gates before
// the first successful load — once onboarding has data, a transient poll failure
// keeps showing the last-good state (React Query default) — but AuthError always
// gates, even with cached data: FM's admin API is 401-only cookie auth, and a 401
// means the session itself is no longer valid, a fact that stale cached data must
// never be allowed to mask.
//
// A 403 gates for that same reason and is its own state, not a variant of either
// neighbour. It is not `needs-auth`: the session was accepted, so offering a
// sign-in would ask the operator to re-enter a password that is not the problem.
// It is not `daemon-unreachable`: the daemon is running and answered. And it is
// not `ready`: the call the whole dashboard is built on was refused, so letting
// the routed tree mount would render a shell whose every panel fails on its own,
// with nothing on the screen saying why.
export const useBootStatus = (): BootState => {
  const onboarding = useOnboarding();
  const queryClient = useQueryClient();

  // Booting is latched to "the daemon has never answered", not to the query being
  // in flight. Deriving it from `isPending` unmounted the whole tree whenever that
  // query went back to pending — and the components this gate guards observe the
  // same query, so unmounting them provoked the next fetch and the gate flapped
  // until the browser's connection pool jammed. Having been answered once is a
  // fact that cannot become false.
  const [hasAnswered, setHasAnswered] = useState(false);
  // Guarded setState during render, not an effect — the compiler forbids setState
  // inside useEffect, so this is the sanctioned "adjust state on data change" shape.
  if (!hasAnswered && (onboarding.data !== undefined || onboarding.error !== null)) {
    setHasAnswered(true);
  }

  const needsAuth = onboarding.error instanceof AuthError;
  const accessDenied = onboarding.error instanceof AccessDeniedError;
  const daemonUnreachable = !onboarding.data && isDaemonUnreachable(onboarding.error);

  // Both gates clear the cache below for the same reason: the session may no
  // longer read what those queries hold, so their values must not flash back.
  const gated = hasAnswered && (needsAuth || accessDenied);

  // Once gated, drop cached data for every OTHER privileged query so stale values
  // can't flash back in if the gate ever lifts without a fresh fetch. The
  // onboarding query itself is excluded: its errored state is what keeps this gate
  // mounted, and removing it would immediately provoke a refetch and flip status
  // away from needs-auth before the operator re-authenticates (a refetch loop).
  // Compare by hashKey, never by reference.
  useEffect(() => {
    if (!gated) return;
    queryClient.removeQueries({
      predicate: (query) => hashKey(query.queryKey) !== hashKey(ONBOARDING_KEY)
    });
  }, [gated, queryClient]);

  // Mirror image of the removal above: once a previously-gated boot query succeeds
  // again (a real re-login, not just a stray render), refresh everything so the
  // privileged data swept away above comes back fresh rather than staying missing
  // until each page happens to remount.
  const wasGated = useRef(false);
  useEffect(() => {
    if (wasGated.current && !gated && onboarding.isSuccess) {
      queryClient.invalidateQueries();
    }
    wasGated.current = gated;
  }, [gated, onboarding.isSuccess, queryClient]);

  const onRetry = () => {
    onboarding.refetch();
  };

  const failure = onboarding.error;

  if (!hasAnswered) return { status: 'booting', failure, onRetry };
  if (needsAuth) return { status: 'needs-auth', failure, onRetry };
  if (accessDenied) return { status: 'access-denied', failure, onRetry };
  if (daemonUnreachable) return { status: 'daemon-unreachable', failure, onRetry };

  return { status: 'ready', failure, onRetry };
};
