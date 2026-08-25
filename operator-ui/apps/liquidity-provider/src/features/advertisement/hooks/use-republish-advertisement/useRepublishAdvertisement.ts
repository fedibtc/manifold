import type {
  RepublishAdvertisementRequest,
  RepublishAdvertisementResponse
} from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { ADVERTISEMENT_KEY } from '@/shared/api/queryKeys';

export const useRepublishAdvertisement = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () =>
      adminCall<RepublishAdvertisementRequest, RepublishAdvertisementResponse>(
        'republish_advertisement',
        { force: true }
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ADVERTISEMENT_KEY })
  });
};
