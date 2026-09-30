import type { OnboardingResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { ONBOARDING_KEY } from '@/shared/api/hooks/use-onboarding/useOnboarding';

// Holder relay access is operator-driven during setup and later renewal. This is
// a mutation, not a query on the Onboarding key: a refetch or invalidation of
// that key reruns its last query function, which would repeat this relay read
// without the operator asking. The answer is an Onboarding response, so it
// replaces the cached one directly.
export const useRefreshAuthorization = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: () => adminCall<OnboardingResponse>('RefreshHolderAuthorizations'),
    onSuccess: (data) => {
      queryClient.setQueryData(ONBOARDING_KEY, data);
    }
  });
};
