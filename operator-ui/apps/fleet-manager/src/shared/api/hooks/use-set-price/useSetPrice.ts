import type { SetPriceResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { OFFER_KEY } from '@/shared/api/hooks/use-offer/useOffer';

export const useSetPrice = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (priceMsat: number | null) =>
      adminCall<SetPriceResponse>({ SetPrice: { price_msats: priceMsat } }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: OFFER_KEY });
    }
  });
};
