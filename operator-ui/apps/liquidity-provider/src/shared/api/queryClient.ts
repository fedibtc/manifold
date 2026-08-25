import { hashKey, QueryCache, QueryClient } from '@tanstack/react-query';
import { AuthError } from './errors';
import { SETUP_STATE_KEY } from './hooks/use-setup-state/useSetupState';

// A 401 from any privileged query means the operator's credential itself is
// dead, not that one route is unhappy — every other query is about to fail the
// same way. Without this, the screen that took the 401 renders a local error and
// the re-auth gate does not appear until the boot poll next runs, up to
// POLL_SETUP_MS later. Nudging the boot query closes that window: it takes the
// same 401 and raises the gate on the next tick, through the one code path that
// already owns gating.
//
// 403 deliberately does NOT escalate. Per SPEC-flip-admin-api.md:31-33 it is an
// authenticated request denied by policy — a fact about that route, which says
// nothing about the operator's access to the rest of the app. Escalating it
// would lock an operator out of screens they are still entitled to use.
const promoteAuthFailure = (error: unknown, queryKey: readonly unknown[]): void => {
  if (!(error instanceof AuthError)) return;
  // The boot query's own 401 is already the gate's input; refetching it from
  // here would only spin.
  if (hashKey(queryKey) === hashKey(SETUP_STATE_KEY)) return;
  void queryClient.refetchQueries({ queryKey: SETUP_STATE_KEY });
};

export const queryClient = new QueryClient({
  queryCache: new QueryCache({
    onError: (error, query) => promoteAuthFailure(error, query.queryKey)
  })
});
