import type { GuardianLinkTestResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { GUARDIAN_LINK_KEY } from '@/features/phone-link/api/hooks/use-guardian-link/useGuardianLink';
import { adminCall } from '@/shared/api/adminCall';

export const useTestGuardianLinkNotification = () => {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (_linkedAt: number) =>
      adminCall<GuardianLinkTestResponse>('TestGuardianLinkNotification'),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: GUARDIAN_LINK_KEY });
    }
  });
};
