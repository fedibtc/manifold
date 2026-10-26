import type { SetAutoUpdateResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { AUTO_UPDATE_KEY } from '@/features/auto-update/api/hooks/use-auto-update/useAutoUpdate';
import { adminCall } from '@/shared/api/adminCall';

// SetAutoUpdate answers with the stored view, in the shape ShowAutoUpdate reads
// it back, so the answer seeds the cache directly.
export const useSetAutoUpdate = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (enabled: boolean) =>
      adminCall<SetAutoUpdateResponse>({ SetAutoUpdate: { enabled } }),
    onSuccess: (stored) => {
      queryClient.setQueryData(AUTO_UPDATE_KEY, stored);
    }
  });
};
