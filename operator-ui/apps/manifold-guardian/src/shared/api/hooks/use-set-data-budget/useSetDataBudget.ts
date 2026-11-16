import type { MonthlyDataBudget, SetDataBudgetResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { CAPACITY_KEY } from '@/shared/api/hooks/use-capacity/useCapacity';

/** Records the operator's answer: `unlimited`, or the GB as a token. */
export const useSetDataBudget = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (monthlyDataBudget: Exclude<MonthlyDataBudget, 'not_set'>) =>
      adminCall<SetDataBudgetResponse>({
        SetDataBudget: { monthly_data_budget: monthlyDataBudget }
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: CAPACITY_KEY });
    }
  });
};
