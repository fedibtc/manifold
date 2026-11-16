import type { ConfigureInitialOfferResponse, MonthlyDataBudget } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { ONBOARDING_KEY } from '@/shared/api/hooks/use-onboarding/useOnboarding';

interface InitialOffer {
  /** null is automatic. */
  maxSeats: number | null;
  priceMsat: number | null;
  /** The operator's answer: setup does not finish without one. */
  monthlyDataBudget: Exclude<MonthlyDataBudget, 'not_set'>;
}

export const useConfigureInitialOffer = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ maxSeats, priceMsat, monthlyDataBudget }: InitialOffer) =>
      adminCall<ConfigureInitialOfferResponse>({
        ConfigureInitialOffer: {
          max_seats: maxSeats,
          price_msats: priceMsat,
          monthly_data_budget: monthlyDataBudget
        }
      }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ONBOARDING_KEY })
  });
};
