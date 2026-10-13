import type { ConfigureInitialOfferResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { ONBOARDING_KEY } from '@/shared/api/hooks/use-onboarding/useOnboarding';

interface InitialOffer {
  maxSeats: number;
  priceMsat: number | null;
}

export const useConfigureInitialOffer = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ maxSeats, priceMsat }: InitialOffer) =>
      adminCall<ConfigureInitialOfferResponse>({
        ConfigureInitialOffer: { max_seats: maxSeats, price_msats: priceMsat }
      }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ONBOARDING_KEY })
  });
};
