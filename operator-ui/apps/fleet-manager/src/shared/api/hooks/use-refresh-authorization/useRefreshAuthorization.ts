import type { OnboardingResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { ONBOARDING_KEY } from '@/shared/api/hooks/use-onboarding/useOnboarding';

// Holder relay access is operator-driven during setup and later renewal. This is
// a mutation, not a query on the Onboarding key: a refetch or invalidation of
// that key reruns its last query function, which would repeat this relay read
// without the operator asking. The answer is an Onboarding response, so it
// replaces the cached one directly, after cancelling any Onboarding read still
// in flight: that read predates the refresh and would otherwise overwrite it.
// Cancelling with the default revert restores the pre-fetch state
// synchronously; `revert: false` would leave the query in an error state.
export const useRefreshAuthorization = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: () => adminCall<OnboardingResponse>('RefreshHolderAuthorizations'),
    onSuccess: (data) => {
      void queryClient.cancelQueries({ queryKey: ONBOARDING_KEY });
      queryClient.setQueryData(ONBOARDING_KEY, data);
    }
  });
};
