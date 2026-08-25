import { SelectField, TextInput } from '@operator-ui/common-ui';
import type { ChainObserverBackend } from '@operator-ui/types';
import type { StepProps } from '@/features/setup/types';
import styles from './ChainObserverStep.module.css';

type BitcoindBackend = Extract<ChainObserverBackend, { type: 'bitcoind' }>;

const BACKEND_OPTIONS = [
  { value: 'esplora', label: 'Esplora' },
  { value: 'bitcoind', label: 'Bitcoin Core (bitcoind)' }
];

const backendHint = 'Source the provider watches for on-chain confirmations.';

const passwordHint = 'Optional. Write-only — leave blank to keep the stored one.';

export const ChainObserverStep = ({ draft, onChange, errors }: StepProps) => {
  const { backend } = draft.chain_observer;
  const setBackend = (next: ChainObserverBackend) => {
    onChange({ chain_observer: { backend: next } });
  };
  const handleType = (value: string) => {
    if (value === 'bitcoind') {
      setBackend({ type: 'bitcoind', url: '', username: '' });
      return;
    }
    setBackend({ type: 'esplora', url: '' });
  };
  const handleUrl = (value: string) => {
    if (backend.type === 'bitcoind') {
      setBackend({ ...backend, url: value });
      return;
    }
    setBackend({ type: 'esplora', url: value });
  };
  const handleBitcoindField = (patch: Partial<BitcoindBackend>) => {
    if (backend.type !== 'bitcoind') {
      return;
    }
    setBackend({ ...backend, ...patch });
  };
  const handleUsername = (value: string) => handleBitcoindField({ username: value });
  // The password is not a config field. It is stored by name and a config write
  // cannot touch it, so it lives beside the draft rather than inside it — which
  // is what lets a blank box mean "unchanged" instead of "delete it".
  const handlePassword = (value: string) =>
    onChange({ secrets: { ...draft.secrets, chainObserverPassword: value } });
  return (
    <div className={styles.layout}>
      <SelectField
        label="Backend"
        value={backend.type}
        onChange={handleType}
        options={BACKEND_OPTIONS}
        hint={backendHint}
      />

      <TextInput
        label="URL"
        value={backend.url}
        onChange={handleUrl}
        placeholder="https://mempool.space/signet/api"
        error={errors.url}
      />

      {backend.type === 'bitcoind' && (
        <>
          <TextInput
            label="Username"
            value={backend.username ?? ''}
            onChange={handleUsername}
            placeholder="Optional"
          />

          <TextInput
            label="Password"
            type="password"
            value={draft.secrets.chainObserverPassword}
            onChange={handlePassword}
            hint={passwordHint}
          />
        </>
      )}
    </div>
  );
};
