import type { GetProviderConfigResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';

export const PROVIDER_CONFIG_KEY = ['provider-config'] as const;

// Settings page source of truth. retry:false surfaces AuthError/NetworkError
// immediately instead of retrying behind a spinner; staleTime mirrors
// useSetupState so a route change does not force an unnecessary refetch.
export const useProviderConfig = () =>
  useQuery({
    retry: false,
    staleTime: 55_000,
    queryKey: PROVIDER_CONFIG_KEY,
    queryFn: () => adminCall<null, GetProviderConfigResponse>('get_provider_config', null)
  });
