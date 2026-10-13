import { afterEach, vi } from 'vitest';
import { AdminApiError, AuthError } from '../errors';
import { ONBOARDING_KEY } from '../hooks/use-onboarding/useOnboarding';
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

it('should set a 15s app-wide staleTime that hooks can override', () => {
  expect(queryClient.getDefaultOptions().queries?.staleTime).toBe(15_000);
});

it('should leave refetchIntervalInBackground at its default of false', () => {
  expect(queryClient.getDefaultOptions().queries?.refetchIntervalInBackground).toBeUndefined();
});

it('should refetch the boot query when any other query takes a 401', async () => {
  const refetch = vi.spyOn(queryClient, 'refetchQueries').mockResolvedValue(undefined);

  await failQuery(['seats'], new AuthError());

  expect(refetch).toHaveBeenCalledWith({ queryKey: ONBOARDING_KEY });
});

it('should not refetch the boot query when the boot query itself takes the 401', async () => {
  const refetch = vi.spyOn(queryClient, 'refetchQueries').mockResolvedValue(undefined);

  await failQuery(ONBOARDING_KEY, new AuthError());

  expect(refetch).not.toHaveBeenCalled();
});

it('should leave a non-auth failure local to the query that took it', async () => {
  const refetch = vi.spyOn(queryClient, 'refetchQueries').mockResolvedValue(undefined);

  await failQuery(['seats'], new AdminApiError('boom'));

  expect(refetch).not.toHaveBeenCalled();
});
