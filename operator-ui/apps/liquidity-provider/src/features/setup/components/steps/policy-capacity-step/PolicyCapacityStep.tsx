import {
  Banner,
  Button,
  CheckboxField,
  FormField,
  SelectField,
  TextInput
} from '@operator-ui/common-ui';
import type {
  AcceptedAttesterPolicy,
  CapacityConfig,
  CapacityMode,
  ReplenishmentConfig,
  SourceType,
  VerificationRequirement
} from '@operator-ui/types';
import type { StepProps } from '@/features/setup/types';
import styles from './PolicyCapacityStep.module.css';

const CAPACITY_MODE_OPTIONS = [
  { value: 'available_funds', label: 'Track available funds' },
  { value: 'explicit_cap', label: 'Explicit cap' }
];

const VERIFICATION_OPTIONS = [
  { value: 'all_trusted', label: 'All trusted' },
  { value: 'consensus_majority_trusted', label: 'Consensus majority trusted' }
];

const parseSats = (value: string): number => {
  const parsed = Number(value);
  return Number.isNaN(parsed) ? 0 : parsed;
};

export const PolicyCapacityStep = ({ draft, onChange, errors }: StepProps) => {
  const { capacity, replenishment, policy } = draft;

  const updateCapacity = (patch: Partial<CapacityConfig>) => {
    onChange({ capacity: { ...capacity, ...patch } });
  };
  const handleMode = (value: string) => updateCapacity({ mode: value as CapacityMode });
  const handleCap = (value: string) => updateCapacity({ explicit_cap: parseSats(value) });
  const toggleSource = (source: SourceType, checked: boolean) => {
    const next = checked
      ? [...capacity.supported_sources, source]
      : capacity.supported_sources.filter((entry) => entry !== source);
    updateCapacity({ supported_sources: next });
  };
  const handleGatewaySource = (checked: boolean) => toggleSource('gateway', checked);
  const handleStabilitySource = (checked: boolean) => toggleSource('stability_pool', checked);

  const updateReplenishment = (patch: Partial<ReplenishmentConfig>) => {
    onChange({ replenishment: { ...replenishment, ...patch } });
  };
  const handleWarning = (value: string) =>
    updateReplenishment({ warning_threshold: parseSats(value) });
  const handleCritical = (value: string) =>
    updateReplenishment({ critical_threshold: parseSats(value) });

  // Only the attester list. `supported_networks` used to be written here too,
  // as a side effect, which is why it drifted: it is derived from the network
  // and the network is edited on another step, so editing the network alone
  // left the two disagreeing and the daemon refusing every request. It is
  // derived where the network is chosen instead — see NetworkStep.
  const updateAttesters = (next: AcceptedAttesterPolicy[]) => {
    onChange({
      policy: { ...policy, accepted_attester_policies: next }
    });
  };
  const updateAttester = (index: number, patch: Partial<AcceptedAttesterPolicy>) => {
    const next = policy.accepted_attester_policies.map((entry, i) =>
      i === index ? { ...entry, ...patch } : entry
    );
    updateAttesters(next);
  };
  const removeAttester = (index: number) => {
    updateAttesters(policy.accepted_attester_policies.filter((_, i) => i !== index));
  };
  const addAttester = () => {
    updateAttesters([
      ...policy.accepted_attester_policies,
      { attester_pubkey: '', verification_requirement: 'all_trusted' }
    ]);
  };

  const capOverWarning = replenishment.critical_threshold > replenishment.warning_threshold;
  const capValue = capacity.explicit_cap != null ? String(capacity.explicit_cap) : '';
  return (
    <div className={styles.layout}>
      <section className={styles.section}>
        <h2 className={styles.sectionTitle}>Capacity</h2>

        <SelectField
          label="Capacity mode"
          value={capacity.mode}
          onChange={handleMode}
          options={CAPACITY_MODE_OPTIONS}
          hint="How the advertised liquidity ceiling is determined."
        />

        {capacity.mode === 'explicit_cap' ? (
          <TextInput
            label="Cap amount (SATS)"
            value={capValue}
            onChange={handleCap}
            placeholder="0"
            error={errors.explicit_cap}
          />
        ) : null}

        <FormField label="Supported sources" error={errors.supported_sources}>
          {() => (
            <div className={styles.sources}>
              <CheckboxField
                label="Gateway"
                checked={capacity.supported_sources.includes('gateway')}
                onChange={handleGatewaySource}
              />

              <CheckboxField
                label="Stability pool"
                checked={capacity.supported_sources.includes('stability_pool')}
                onChange={handleStabilitySource}
              />
            </div>
          )}
        </FormField>
      </section>

      <section className={styles.section}>
        <h2 className={styles.sectionTitle}>Alerts</h2>

        <TextInput
          label="Low-balance warning (SATS)"
          value={String(replenishment.warning_threshold)}
          onChange={handleWarning}
          error={errors.warning_threshold}
        />

        <TextInput
          label="Critical threshold (SATS)"
          value={String(replenishment.critical_threshold)}
          onChange={handleCritical}
          error={errors.critical_threshold}
        />

        {capOverWarning ? (
          <Banner variant="warn" title="Check your thresholds">
            The critical threshold is higher than the warning threshold — warnings will never fire
            before critical.
          </Banner>
        ) : null}
      </section>

      <section className={styles.section}>
        <h2 className={styles.sectionTitle}>Trusted attesters</h2>

        <FormField
          label="Accepted attester policies"
          hint="Attesters whose credentials this provider will accept."
          error={errors.accepted_attester_policies}
        >
          {() => (
            <div className={styles.attesterList}>
              {policy.accepted_attester_policies.map((entry, index) => (
                // biome-ignore lint/suspicious/noArrayIndexKey: rows are positional, no stable id available
                <div key={index} className={styles.attesterRow}>
                  <div className={styles.attesterInput}>
                    <TextInput
                      label={`Attester ${index + 1} pubkey`}
                      value={entry.attester_pubkey}
                      onChange={(value) => updateAttester(index, { attester_pubkey: value })}
                      placeholder="npub…"
                    />
                  </div>

                  <div className={styles.attesterInput}>
                    <SelectField
                      label="Verification"
                      value={entry.verification_requirement}
                      onChange={(value) =>
                        updateAttester(index, {
                          verification_requirement: value as VerificationRequirement
                        })
                      }
                      options={VERIFICATION_OPTIONS}
                    />
                  </div>

                  <button
                    type="button"
                    className={styles.remove}
                    onClick={() => removeAttester(index)}
                    aria-label={`Remove attester ${index + 1}`}
                  >
                    ×
                  </button>
                </div>
              ))}
              <div>
                <Button variant="secondary" size="small" onClick={addAttester}>
                  Add attester
                </Button>
              </div>
            </div>
          )}
        </FormField>
      </section>
    </div>
  );
};
