import type { QueryClient } from '@tanstack/react-query';
import { fetchOnboarding, ONBOARDING_KEY } from '@/shared/api/hooks/use-onboarding/useOnboarding';

/**
 * Run after the daemon installs an identity — a new fleet or a recovered one.
 *
 * Every cached answer describes the host as it was before, so the wizard must not
 * read one to decide its next step. This waits for a single fresh `Onboarding`
 * before it returns, so whoever awaits the mutation sees the identity the daemon
 * just installed rather than the one it replaced.
 *
 * The call is made directly rather than through `fetchQuery`, and this is the
 * whole point of the function: react-query answers a fetch with whatever request
 * is already in flight for that key, and `Onboarding` polls every few seconds
 * throughout setup. A `fetchQuery` here would frequently resolve with a reading
 * taken before the identity changed — exactly the stale answer being fixed.
 *
 * A failed fetch resets the cached answer instead of keeping it: the identity is
 * installed either way, and a stale reading is worse than none. `resetQueries`
 * also refetches the live observers, which is the retry this case wants.
 */
export const refreshIdentityQueries = async (queryClient: QueryClient): Promise<void> => {
  try {
    queryClient.setQueryData(ONBOARDING_KEY, await fetchOnboarding());
  } catch {
    await queryClient.resetQueries({ queryKey: ONBOARDING_KEY, exact: true });
  }

  await queryClient.invalidateQueries();
};
