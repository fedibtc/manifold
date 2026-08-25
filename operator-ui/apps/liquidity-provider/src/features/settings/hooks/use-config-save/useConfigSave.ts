import type {
  ApplySetupConfigRequest,
  ApplySetupConfigResponse,
  SetupConfig,
  SetupValidationSummary,
  ValidateSetupRequest,
  ValidateSetupResponse
} from '@operator-ui/types';
import { useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useUpdateProviderConfig } from '@/features/settings/hooks/use-update-provider-config/useUpdateProviderConfig';
import { buildProviderConfigPatch, hasHardFieldChange } from '@/features/settings/utils/configDiff';
import { adminCall } from '@/shared/api/adminCall';
import { SETUP_STATE_KEY } from '@/shared/api/hooks/use-setup-state/useSetupState';
import type { ConfigDraft } from '@/shared/config/draft';
import { storeDraftSecrets, toSetupConfig } from '@/shared/config/secrets';

type SavePhase = 'idle' | 'saving' | 'success' | 'validation_failed' | 'error';

interface ConfigSaveOutcome {
  status: 'success' | 'validation_failed' | 'error';
  validation: SetupValidationSummary | null;
  error?: Error;
}

// Save orchestrator for the settings page. Soft-only edits patch through
// update_provider_config; a hard-field edit (network/gateway/chain
// observer/credential locations) goes through the full validate_setup +
// apply_setup_config round trip, because those fields are validated against
// their live dependencies before being accepted. Both take effect on the
// running daemon — dependency config is re-read from storage per worker pass,
// so neither path needs a restart.
export const useConfigSave = () => {
  const queryClient = useQueryClient();
  const updateProviderConfig = useUpdateProviderConfig();
  const [phase, setPhase] = useState<SavePhase>('idle');
  const [validation, setValidation] = useState<SetupValidationSummary | null>(null);

  const save = async (baseline: SetupConfig, draft: ConfigDraft): Promise<ConfigSaveOutcome> => {
    setPhase('saving');
    setValidation(null);
    try {
      // Before the config write either way: the daemon validates a candidate
      // against the stored secrets, so a credential the operator just typed has
      // to be stored before it can be tested. This is what replaced the old
      // credential guard — a config write cannot touch a secret now, so there
      // is nothing left to refuse a save over.
      await storeDraftSecrets(draft);
      const config = toSetupConfig(draft);

      if (hasHardFieldChange(baseline, draft)) {
        const validateResponse = await adminCall<ValidateSetupRequest, ValidateSetupResponse>(
          'validate_setup',
          { candidate_config: config }
        );
        if (validateResponse.validation.status !== 'passed') {
          setValidation(validateResponse.validation);
          setPhase('validation_failed');
          return {
            status: 'validation_failed',
            validation: validateResponse.validation
          };
        }

        const applyResponse = await adminCall<ApplySetupConfigRequest, ApplySetupConfigResponse>(
          'apply_setup_config',
          { config }
        );
        setValidation(applyResponse.validation);
        if (applyResponse.status !== 'ready') {
          setPhase('validation_failed');
          return {
            status: 'validation_failed',
            validation: applyResponse.validation
          };
        }

        await queryClient.invalidateQueries({ queryKey: SETUP_STATE_KEY });
        setPhase('success');
        return { status: 'success', validation: applyResponse.validation };
      }

      const patch = buildProviderConfigPatch(baseline, config);
      await updateProviderConfig.mutateAsync(patch);
      setPhase('success');
      return { status: 'success', validation: null };
    } catch (err) {
      const error = err instanceof Error ? err : new Error('Something went wrong. Try again.');
      setPhase('error');
      return { status: 'error', validation: null, error };
    }
  };

  return { save, phase, validation, isSaving: phase === 'saving' };
};
