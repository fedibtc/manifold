import type { OnboardingResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { ONBOARDING_KEY } from '@/shared/api/hooks/use-onboarding/useOnboarding';

// Holder relay access is operator-driven during onboarding. The shared cached
// Onboarding value renders immediately; only "Check now" calls this disabled
// query's refetch and performs a bounded relay reconciliation.
export const useAuthorizationWatch = () =>
  useQuery({
    enabled: false,
    retry: false,
    queryKey: ONBOARDING_KEY,
    queryFn: (): Promise<OnboardingResponse> =>
      adminCall<OnboardingResponse>('RefreshHolderAuthorizations')
  });
