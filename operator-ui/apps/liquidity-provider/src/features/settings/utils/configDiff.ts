import type { ProviderConfigPatch, ProviderDisplayPatch, SetupConfig } from '@operator-ui/types';

// Fields validated against their live dependencies before being accepted, so
// they go through the full validate_setup + apply_setup_config round trip
// rather than a patch. They take effect on the running daemon like everything
// else — it re-reads dependency config from storage each worker pass.
export const HARD_FIELDS = ['network', 'gateway', 'chain_observer'] as const;

// Fields that patch through update_provider_config.
export const SOFT_FIELDS = [
  'policy',
  'relays',
  'capacity',
  'funding_policy',
  'replenishment',
  'advertised_endpoint',
  'advertisement',
  'provider_display'
] as const;

const deepEqual = (a: unknown, b: unknown): boolean => JSON.stringify(a) === JSON.stringify(b);

export const hasHardFieldChange = (baseline: SetupConfig, draft: SetupConfig): boolean =>
  HARD_FIELDS.some((field) => !deepEqual(baseline[field], draft[field]));

const buildProviderDisplayPatch = (
  baseline: SetupConfig['provider_display'],
  draft: SetupConfig['provider_display']
): ProviderDisplayPatch | null => {
  if (deepEqual(baseline ?? null, draft ?? null)) {
    return null;
  }
  return draft ? { action: 'set', value: draft } : { action: 'clear' };
};

// Only the soft fields that differ from the baseline snapshot go in the
// patch — an unchanged field is omitted entirely, not sent as its current
// value, so update_provider_config never registers a false change.
export const buildProviderConfigPatch = (
  baseline: SetupConfig,
  draft: SetupConfig
): ProviderConfigPatch => {
  const patch: ProviderConfigPatch = {};
  if (!deepEqual(baseline.policy, draft.policy)) patch.policy = draft.policy;
  if (!deepEqual(baseline.relays, draft.relays)) patch.relays = draft.relays;
  if (!deepEqual(baseline.capacity, draft.capacity)) patch.capacity = draft.capacity;
  if (!deepEqual(baseline.funding_policy, draft.funding_policy)) {
    patch.funding_policy = draft.funding_policy;
  }
  if (!deepEqual(baseline.replenishment, draft.replenishment)) {
    patch.replenishment = draft.replenishment;
  }
  if (!deepEqual(baseline.advertised_endpoint, draft.advertised_endpoint)) {
    patch.advertised_endpoint = draft.advertised_endpoint;
  }
  if (!deepEqual(baseline.advertisement, draft.advertisement)) {
    patch.advertisement = draft.advertisement;
  }

  const providerDisplayPatch = buildProviderDisplayPatch(
    baseline.provider_display,
    draft.provider_display
  );
  if (providerDisplayPatch) {
    patch.provider_display = providerDisplayPatch;
  }

  return patch;
};
