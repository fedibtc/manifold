import { Banner, SelectField } from '@operator-ui/common-ui';
import type { BitcoinNetwork } from '@operator-ui/types';
import type { StepProps } from '@/features/setup/types';
import styles from './NetworkStep.module.css';

const NETWORK_OPTIONS = [
  { value: 'signet', label: 'Signet' },
  { value: 'bitcoin', label: 'Bitcoin' },
  { value: 'testnet', label: 'Testnet' },
  { value: 'regtest', label: 'Regtest' }
];

const networkHint = 'The Bitcoin network this provider operates on.';

export const NetworkStep = ({ draft, onChange, errors }: StepProps) => {
  // `policy.supported_networks` is derived, never asked for. It gates every
  // public request the daemon accepts and every advertisement an FI keeps, and
  // a provider serves the one network it is configured for, so there is nothing
  // for the operator to choose. Deriving it here keeps it in step with the
  // field it depends on; the daemon refuses a config where the two disagree.
  const handleNetwork = (value: string) => {
    const network = value as BitcoinNetwork;
    onChange({
      network,
      policy: { ...draft.policy, supported_networks: [network] }
    });
  };
  return (
    <div className={styles.layout}>
      <SelectField
        label="Network"
        value={draft.network}
        onChange={handleNetwork}
        options={NETWORK_OPTIONS}
        hint={networkHint}
        error={errors.network}
      />

      <p className={styles.serving}>Serving: {draft.policy.supported_networks.join(', ')}</p>

      <Banner variant="info" title="Network is checked against your gateway">
        Saving re-validates that the gateway wallet runs on this network before it takes effect.
      </Banner>
    </div>
  );
};
