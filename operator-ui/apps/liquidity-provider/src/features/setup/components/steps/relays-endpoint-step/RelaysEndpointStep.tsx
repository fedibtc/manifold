import { Button, CheckboxField, FormField, TextInput } from '@operator-ui/common-ui';
import type { StepProps } from '@/features/setup/types';
import styles from './RelaysEndpointStep.module.css';

const relaysHint = 'Nostr relays used to publish and discover advertisements.';
// The Iroh node id is derived from the provider identity, so the daemon owns
// this value and fills it in once the transport binds.
const addressPlaceholder = 'Set by the daemon once the transport binds';
const noop = () => {};
// The published advertisement expires at twice the interval, so the operator is
// told the consequence of the number rather than left to derive it.
const intervalHint = (seconds: number) =>
  seconds > 0
    ? `Advertisement expires after ${Math.round((seconds * 2) / 60)} minutes.`
    : 'How often the advertisement is republished. Must be greater than zero.';

export const RelaysEndpointStep = ({ draft, onChange, errors }: StepProps) => {
  const updateRelay = (index: number, value: string) => {
    const next = draft.relays.map((relay, i) => (i === index ? value : relay));
    onChange({ relays: next });
  };
  const removeRelay = (index: number) => {
    onChange({ relays: draft.relays.filter((_, i) => i !== index) });
  };
  const addRelay = () => {
    onChange({ relays: [...draft.relays, ''] });
  };
  const handleInterval = (value: string) => {
    onChange({
      advertisement: { ...draft.advertisement, republish_interval: Number(value) }
    });
  };
  const handleReady = (checked: boolean) => {
    onChange({
      advertisement: { ...draft.advertisement, ready_advertisement_enabled: checked }
    });
  };
  return (
    <div className={styles.layout}>
      <FormField label="Relays" hint={relaysHint} error={errors.relays}>
        {() => (
          <div className={styles.relayList}>
            {draft.relays.map((relay, index) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: rows are positional, no stable id available
              <div key={index} className={styles.relayRow}>
                <div className={styles.relayInput}>
                  <TextInput
                    label={`Relay ${index + 1}`}
                    value={relay}
                    onChange={(value) => updateRelay(index, value)}
                    placeholder="wss://…"
                  />
                </div>

                <button
                  type="button"
                  className={styles.remove}
                  onClick={() => removeRelay(index)}
                  aria-label={`Remove relay ${index + 1}`}
                >
                  ×
                </button>
              </div>
            ))}
            <div>
              <Button variant="secondary" size="small" onClick={addRelay}>
                Add relay
              </Button>
            </div>
          </div>
        )}
      </FormField>

      <TextInput
        label="Advertised address"
        value={draft.advertised_endpoint.address}
        onChange={noop}
        placeholder={addressPlaceholder}
        disabled
      />

      <TextInput label="Transport" value="Iroh" onChange={noop} disabled />

      <TextInput
        label="RPC protocol"
        value={draft.advertised_endpoint.rpc_protocol_name}
        onChange={noop}
        disabled
      />

      <TextInput
        label="Republish interval (seconds)"
        value={String(draft.advertisement.republish_interval)}
        onChange={handleInterval}
        type="number"
        min={1}
        hint={intervalHint(draft.advertisement.republish_interval)}
        error={errors.republish_interval}
      />

      <CheckboxField
        label="Advertise readiness"
        checked={draft.advertisement.ready_advertisement_enabled}
        onChange={handleReady}
        hint="Publish a ready advertisement once setup is applied."
      />
    </div>
  );
};
