import type { CollectGuardianFeesResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { guardianFeesKey } from '@/shared/api/hooks/use-guardian-fees/useGuardianFees';

// Step one of two. This moves what the pool will release into ordinary ecash; it
// does not send anything anywhere. Locked deposits leave only at the next cycle
// turnover, which is why the answer carries `awaiting_cycle_msat` alongside
// `claimed_msat` (crates/fman/core/src/fleet.rs:1219-1227).
export const useCollectGuardianFees = (seatId: string) => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: () =>
      adminCall<CollectGuardianFeesResponse>({ CollectGuardianFees: { seat_id: seatId } }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: guardianFeesKey(seatId) });
    }
  });
};
