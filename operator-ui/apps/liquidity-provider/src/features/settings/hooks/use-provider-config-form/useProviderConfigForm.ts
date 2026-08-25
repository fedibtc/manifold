import { type QueryDisposition, useQueryDisposition } from '@operator-ui/common-ui';
import type { SetupValidationSummary } from '@operator-ui/types';
import { useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useProviderConfig } from '@/features/settings/api/hooks/use-provider-config/useProviderConfig';
import { useConfigSave } from '@/features/settings/hooks/use-config-save/useConfigSave';
import { seedDraftFromView } from '@/features/settings/utils/seedDraftFromView';
import { ADVERTISEMENT_KEY } from '@/shared/api/queryKeys';
import type { ConfigDraft } from '@/shared/config/draft';

type ConfigSave = ReturnType<typeof useConfigSave>;
type ValidationCheck = SetupValidationSummary['checks'][number];

interface FormSurface {
  disposition: QueryDisposition;
  retry: () => void;
}

export type ProviderConfigForm =
  | ({ status: 'pending' } & FormSurface)
  | ({
      status: 'ready';
      draft: ConfigDraft;
      onChange: (patch: Partial<ConfigDraft>) => void;
      save: () => Promise<void>;
      isSaving: boolean;
      phase: ConfigSave['phase'];
      saveError: string | null;
      failedChecks: ValidationCheck[];
    } & FormSurface);

export const useProviderConfigForm = (): ProviderConfigForm => {
  const providerConfig = useProviderConfig();
  const configSave = useConfigSave();
  const queryClient = useQueryClient();
  const [baseline, setBaseline] = useState<ConfigDraft | null>(null);
  const [draft, setDraft] = useState<ConfigDraft | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);

  if (providerConfig.isSuccess && providerConfig.data && baseline === null) {
    const seeded = seedDraftFromView(providerConfig.data.config);
    setBaseline(seeded);
    setDraft(seeded);
  }

  // The draft is the answer this screen holds, so the disposition is read over
  // it rather than over the raw query. The two differ for one render — the
  // draft is seeded from the response during render — and reading the query
  // would claim content the screen cannot yet show.
  //
  // Reading a disposition at all is the point. This used to be
  // `providerConfig.isError || !providerConfig.data`, which called a failed
  // refresh and having nothing the same thing. React Query keeps `data` through
  // a failed refetch, so a refresh that failed over a perfectly good config
  // blanked the whole settings screen, and the operator lost figures the app
  // still held. Now that case is `stale`: the form stays up under a marker.
  const { disposition, retry } = useQueryDisposition([
    {
      data: draft ?? undefined,
      isError: providerConfig.isError,
      error: providerConfig.error,
      dataUpdatedAt: providerConfig.dataUpdatedAt,
      refetch: providerConfig.refetch
    }
  ]);

  if (!draft) return { status: 'pending', disposition, retry };

  const onChange = (patch: Partial<ConfigDraft>) => {
    setDraft((previous) => (previous ? { ...previous, ...patch } : previous));
  };

  const save = async () => {
    if (!baseline) return;
    setSaveError(null);
    const outcome = await configSave.save(baseline, draft);
    if (outcome.status === 'success') {
      queryClient.invalidateQueries({ queryKey: ADVERTISEMENT_KEY });
      setBaseline(draft);
    } else if (outcome.status === 'error') {
      setSaveError(outcome.error?.message ?? 'Something went wrong. Try again.');
    }
  };

  const failedChecks =
    configSave.validation?.checks.filter((check) => check.status !== 'passed') ?? [];
  return {
    status: 'ready',
    disposition,
    retry,
    draft,
    onChange,
    save,
    isSaving: configSave.isSaving,
    phase: configSave.phase,
    saveError,
    failedChecks
  };
};
