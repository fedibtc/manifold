import type { SetCapacityResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { CAPACITY_KEY } from '@/shared/api/hooks/use-capacity/useCapacity';
import { SEATS_KEY } from '@/shared/api/hooks/use-seats/useSeats';

export const useSetCapacity = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (maxSeats: number) =>
      adminCall<SetCapacityResponse>({ SetCapacity: { max_seats: maxSeats } }),
    // The ceiling bounds how many seats may be admitted, so the seat list's own
    // sense of what is still sellable moves with it.
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: CAPACITY_KEY });
      void queryClient.invalidateQueries({ queryKey: SEATS_KEY });
    }
  });
};
