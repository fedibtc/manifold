import type { CreateSeatGrantResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { SEAT_GRANTS_KEY } from '@/features/guardian-codes/api/hooks/use-seat-grants/useSeatGrants';
import { adminCall } from '@/shared/api/adminCall';

export const useCreateSeatGrant = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (label: string | null) =>
      adminCall<CreateSeatGrantResponse>({ CreateSeatGrant: { label } }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: SEAT_GRANTS_KEY });
    }
  });
};
