import type { ProviderConfigPatch, UpdateProviderConfigResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { PROVIDER_CONFIG_KEY } from '@/features/settings/api/hooks/use-provider-config/useProviderConfig';
import { adminCall } from '@/shared/api/adminCall';

// Soft-field save path: update_provider_config hot-reloads the patch. Only the
// provider-config cache is invalidated here — invalidating the
// advertisement-state key would be a cross-feature import, so that
// invalidation happens at the settings page (composition layer) instead.
export const useUpdateProviderConfig = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (patch: ProviderConfigPatch) =>
      adminCall<{ patch: ProviderConfigPatch }, UpdateProviderConfigResponse>(
        'update_provider_config',
        { patch }
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: PROVIDER_CONFIG_KEY })
  });
};
