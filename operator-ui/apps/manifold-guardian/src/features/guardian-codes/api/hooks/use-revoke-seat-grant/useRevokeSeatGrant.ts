import type { RevokeSeatGrantResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { SEAT_GRANTS_KEY } from '@/features/guardian-codes/api/hooks/use-seat-grants/useSeatGrants';
import { adminCall } from '@/shared/api/adminCall';

export const useRevokeSeatGrant = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (grantNonce: string) =>
      adminCall<RevokeSeatGrantResponse>({ RevokeSeatGrant: { grant_nonce: grantNonce } }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: SEAT_GRANTS_KEY });
    }
  });
};
