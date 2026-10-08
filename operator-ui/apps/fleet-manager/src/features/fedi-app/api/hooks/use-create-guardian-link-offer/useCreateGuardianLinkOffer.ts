import type { GuardianLinkResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { GUARDIAN_LINK_KEY } from '@/features/fedi-app/api/hooks/use-guardian-link/useGuardianLink';
import { adminCall } from '@/shared/api/adminCall';

export const useCreateGuardianLinkOffer = () => {
  const client = useQueryClient();
  return useMutation({
    mutationFn: () => adminCall<GuardianLinkResponse>('CreateGuardianLinkOffer'),
    onSuccess: (status) => {
      client.setQueryData(GUARDIAN_LINK_KEY, status);
      void client.invalidateQueries({ queryKey: GUARDIAN_LINK_KEY });
    }
  });
};
