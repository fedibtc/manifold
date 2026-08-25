import type { OnboardFromBackupResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { refreshIdentityQueries } from '@/features/setup/utils/refreshIdentity';
import { adminCall } from '@/shared/api/adminCall';

export interface RestoreInput {
  mnemonic: string;
  acknowledgeOriginalHostIsGone: boolean;
}

export const useOnboardFromBackup = () => {
  const queryClient = useQueryClient();

  return useMutation({
    // The recovery phrase travels in `variables`, which TanStack Query keeps for
    // the mutation's lifetime. gcTime: 0 drops it the moment nothing observes the
    // mutation, so the phrase does not sit in memory after the screen has moved on.
    gcTime: 0,
    mutationFn: ({ mnemonic, acknowledgeOriginalHostIsGone }: RestoreInput) =>
      adminCall<OnboardFromBackupResponse>({
        OnboardFromBackup: {
          mnemonic,
          acknowledge_original_host_is_gone: acknowledgeOriginalHostIsGone
        }
      }),
    onSuccess: () => refreshIdentityQueries(queryClient)
  });
};
