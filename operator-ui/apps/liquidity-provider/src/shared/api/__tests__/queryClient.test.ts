import { afterEach, describe, expect, it, vi } from 'vitest';
import { AccessDeniedError, AdminApiError, AuthError } from '../errors';
import { SETUP_STATE_KEY } from '../hooks/use-setup-state/useSetupState';
import { queryClient } from '../queryClient';

afterEach(() => {
  vi.restoreAllMocks();
  queryClient.clear();
});

// The cache's onError is the only path that turns one query's 401 into an
// app-wide re-auth, so it is exercised through the real cache rather than by
// reaching for the helper behind it.
const failQuery = (queryKey: readonly unknown[], error: Error) =>
  queryClient
    .fetchQuery({ queryKey, queryFn: () => Promise.reject(error), retry: false })
    .catch(() => {});

describe('auth-failure promotion', () => {
  it('should refetch the boot query when any other query takes a 401', async () => {
    const refetch = vi.spyOn(queryClient, 'refetchQueries').mockResolvedValue(undefined);

    await failQuery(['funds'], new AuthError());

    expect(refetch).toHaveBeenCalledWith({ queryKey: SETUP_STATE_KEY });
  });

  it('should not refetch the boot query when the boot query itself takes the 401', async () => {
    const refetch = vi.spyOn(queryClient, 'refetchQueries').mockResolvedValue(undefined);

    await failQuery(SETUP_STATE_KEY, new AuthError());

    expect(refetch).not.toHaveBeenCalled();
  });

  // 403 is a fact about one route, not about the credential — escalating it
  // would lock the operator out of screens they can still use.
  it('should keep a 403 local to the route that was denied', async () => {
    const refetch = vi.spyOn(queryClient, 'refetchQueries').mockResolvedValue(undefined);

    await failQuery(['funds'], new AccessDeniedError('nope'));

    expect(refetch).not.toHaveBeenCalled();
  });

  it('should leave a non-auth failure local to the query that took it', async () => {
    const refetch = vi.spyOn(queryClient, 'refetchQueries').mockResolvedValue(undefined);

    await failQuery(['funds'], new AdminApiError('internal', 'boom'));

    expect(refetch).not.toHaveBeenCalled();
  });
});
