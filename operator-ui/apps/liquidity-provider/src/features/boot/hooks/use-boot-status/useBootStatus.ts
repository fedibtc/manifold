import { hashKey, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef } from 'react';
import { AccessDeniedError, AuthError, NetworkError } from '@/shared/api/errors';
import { HEALTH_KEY, useHealth } from '@/shared/api/hooks/use-health/useHealth';
import { SETUP_STATE_KEY, useSetupState } from '@/shared/api/hooks/use-setup-state/useSetupState';
import { isRestoreMode, startingReason } from '@/shared/api/restoreMode';

export type BootStatus =
  | 'daemon-unreachable'
  | 'restore-mode'
  | 'reloading'
  | 'no-runtime'
  | 'needs-auth'
  | 'access-denied'
  | 'booting'
  | 'ready';

// Boot order: (1) daemon unreachable → G1; (2) restore mode → standalone
// recovery console (checked before auth: a restore-mode daemon has no
// get_setup_state route at all, so setup's own request may fail for reasons
// that have nothing to do with the operator's credentials — the unauthenticated
// health probe is the only signal that's meaningful here); (3) unauthorized →
// G2; (4) access denied → permission screen; (5) ready.
// Errors only gate before the first successful load — once setup-state has data, a
// transient poll failure keeps showing the last-good state (React Query default) —
// EXCEPT AuthError and AccessDeniedError, which always gate: per
// SPEC-flip-admin-api.md:31-33, 401 means the credentials themselves are no
// longer valid and 403 means an authenticated request was denied by policy;
// neither is a fact that stale cached data should be allowed to mask.
export const useBootStatus = (): { status: BootStatus; onRetry: () => void } => {
  const health = useHealth();
  const setup = useSetupState();
  const queryClient = useQueryClient();

  const needsAuth = setup.error instanceof AuthError;
  const accessDenied = setup.error instanceof AccessDeniedError;
  const daemonUnreachable =
    (!health.data && health.isError) || (!setup.data && setup.error instanceof NetworkError);
  const booting =
    (!health.data && health.isPending) || (!setup.data && setup.isPending && !setup.isError);

  const gated = !booting && (needsAuth || accessDenied);

  // Once gated, drop cached data for every OTHER privileged query so stale
  // values can't flash back in if the gate ever lifts without a fresh fetch.
  // The boot queries themselves (setup-state, health) are excluded: setup-state's
  // errored state is what keeps this gate mounted, and removing it — or
  // health, which the gate also reads — would immediately provoke a refetch
  // and flip status away from needs-auth/access-denied before the operator
  // re-authenticates (a refetch loop). Compare by hashKey, never by reference.
  useEffect(() => {
    if (!gated) return;
    queryClient.removeQueries({
      predicate: (query) => {
        const key = hashKey(query.queryKey);
        return key !== hashKey(SETUP_STATE_KEY) && key !== hashKey(HEALTH_KEY);
      }
    });
  }, [gated, queryClient]);

  // Mirror image of the removal above: once a previously-gated boot query
  // succeeds again (a real re-login, not just a stray render), refresh
  // everything so the privileged data swept away above comes back fresh
  // rather than staying missing until each page happens to remount.
  const wasGated = useRef(false);
  useEffect(() => {
    if (wasGated.current && !gated && setup.isSuccess) {
      queryClient.invalidateQueries();
    }
    wasGated.current = gated;
  }, [gated, setup.isSuccess, queryClient]);

  const starting = startingReason(health.data);

  // The waiting screen above is driven by health, which polls while the daemon
  // has no runtime. Setup-state does not: it polls once a minute. So the moment
  // health reports a serving daemon again, setup still holds the failure it
  // took during the wait — and the gate below would show "can't reach the
  // daemon" for up to a minute, immediately after a restore landed. Asking once
  // on the transition closes that window. Same shape as the re-gate refresh
  // above, and for the same reason.
  const wasStarting = useRef(false);
  const refetchSetup = setup.refetch;
  useEffect(() => {
    if (wasStarting.current && !starting) refetchSetup();
    wasStarting.current = starting !== null;
  }, [starting, refetchSetup]);

  const onRetry = () => {
    health.refetch();
    setup.refetch();
  };

  if (booting) return { status: 'booting', onRetry };
  if (isRestoreMode(health.data)) return { status: 'restore-mode', onRetry };
  if (needsAuth) return { status: 'needs-auth', onRetry };
  if (accessDenied) return { status: 'access-denied', onRetry };
  // Before the unreachable gate, and deliberately. In both of these the daemon
  // is answering `/health` and naming what it is doing, while every privileged
  // route refuses because there is no runtime behind it — so setup-state throws
  // a NetworkError and the unreachable gate below would claim the daemon is not
  // answering, over a line reading "GET /health · connection refused", when
  // that exact route is what told us this.
  if (starting) return { status: starting, onRetry };
  if (daemonUnreachable) return { status: 'daemon-unreachable', onRetry };

  return { status: 'ready', onRetry };
};
