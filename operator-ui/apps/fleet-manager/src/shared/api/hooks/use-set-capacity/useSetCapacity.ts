import type { SetCapacityResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { CAPACITY_KEY } from '@/shared/api/hooks/use-capacity/useCapacity';

export const useSetCapacity = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (maxSeats: number) =>
      adminCall<SetCapacityResponse>({ SetCapacity: { max_seats: maxSeats } }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: CAPACITY_KEY });
    }
  });
};
