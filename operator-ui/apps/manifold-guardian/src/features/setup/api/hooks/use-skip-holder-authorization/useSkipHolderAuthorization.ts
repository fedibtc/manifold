import type { SkipHolderAuthorizationResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { ONBOARDING_KEY } from '@/shared/api/hooks/use-onboarding/useOnboarding';

// The operator sets this host up without a badge. The answer is the Onboarding
// response at its next stage, so it replaces the cached one the same way the
// authorization refresh does: after cancelling any Onboarding read still in
// flight, which predates the skip and would otherwise put the wizard back.
export const useSkipHolderAuthorization = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: () => adminCall<SkipHolderAuthorizationResponse>('SkipHolderAuthorization'),
    onSuccess: (data) => {
      void queryClient.cancelQueries({ queryKey: ONBOARDING_KEY });
      queryClient.setQueryData(ONBOARDING_KEY, data);
    }
  });
};
