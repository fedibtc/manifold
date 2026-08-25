import type { RefreshRelaysRequest, RefreshRelaysResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { ADVERTISEMENT_KEY } from '@/shared/api/queryKeys';

export const useRefreshRelays = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () =>
      adminCall<RefreshRelaysRequest, RefreshRelaysResponse>('refresh_relays', null),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ADVERTISEMENT_KEY })
  });
};
