import { hashKey, QueryCache, QueryClient } from '@tanstack/react-query';
import { AuthError } from './errors';
import { ONBOARDING_KEY } from './hooks/use-onboarding/useOnboarding';

// A 401 from any privileged query means the session itself has expired, not that
// one route is unhappy — every other query is about to fail the same way.
// Without this, the screen that took the 401 renders a local error and the
// re-auth gate does not appear until Onboarding next polls. Nudging the boot
// query closes that window: it takes the same 401 and raises the gate on the
// next tick, through the one code path that already owns gating.
const promoteAuthFailure = (error: unknown, queryKey: readonly unknown[]): void => {
  if (!(error instanceof AuthError)) return;
  // The boot query's own 401 is already the gate's input; refetching it from
  // here would only spin.
  if (hashKey(queryKey) === hashKey(ONBOARDING_KEY)) return;
  void queryClient.refetchQueries({ queryKey: ONBOARDING_KEY });
};

// staleTime is a modest app-wide floor; hooks that need a longer or shorter
// window (see shared/api/pollingIntervals.ts) override it per query.
// refetchIntervalInBackground is left at its default (false) ON PURPOSE: a
// hidden tab must stop polling, not keep spending admin-API calls the
// operator cannot see.
export const queryClient = new QueryClient({
  queryCache: new QueryCache({
    onError: (error, query) => promoteAuthFailure(error, query.queryKey)
  }),
  defaultOptions: {
    queries: {
      staleTime: 15_000
    }
  }
});
