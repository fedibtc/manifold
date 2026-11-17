import type { ResetUptimeResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { SEAT_READINESS_KEY } from '@/shared/api/hooks/use-seat-readiness/useSeatReadiness';

/** Forget the uptime history. The daemon reruns its checks at once; the
 *  readiness poll picks up their verdict. */
export const useResetUptime = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: () => adminCall<ResetUptimeResponse>('ResetUptime'),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: SEAT_READINESS_KEY });
    }
  });
};
