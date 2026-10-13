import type { OnboardAsNewResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { refreshIdentityQueries } from '@/features/setup/utils/refreshIdentity';
import { adminCall } from '@/shared/api/adminCall';

export const useOnboardAsNew = () => {
  const queryClient = useQueryClient();

  return useMutation({
    // `if_needed: false` on purpose: the operator asked to start a new fleet, so
    // "this host already has one" is a refusal they need to see, not a success.
    mutationFn: () => adminCall<OnboardAsNewResponse>({ OnboardAsNew: { if_needed: false } }),
    // No gcTime override here: OnboardAsNew carries no secret in its variables.
    onSuccess: () => refreshIdentityQueries(queryClient)
  });
};
