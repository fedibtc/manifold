import { Banner, Button, FormField, KeyValueEditor, TextInput } from '@operator-ui/common-ui';
import type { GatewayConfig } from '@operator-ui/types';
import { useProbeGateway } from '@/features/setup/hooks/use-probe-gateway/useProbeGateway';
import type { StepProps } from '@/features/setup/types';
import { describeActionError } from '@/shared/utils/describeActionError';
import styles from './GatewayStep.module.css';

const credentialHint =
  'Write-only. Stored securely and never shown again — leave blank to keep the stored one.';
const metadataHint = 'Optional key/value pairs published with your gateway identity.';

export const GatewayStep = ({ draft, onChange, errors }: StepProps) => {
  const updateGateway = (patch: Partial<GatewayConfig>) => {
    onChange({ gateway: { ...draft.gateway, ...patch } });
  };
  const handleName = (value: string) => updateGateway({ gateway_name: value });
  const handleAdminUrl = (value: string) => updateGateway({ admin_url: value });
  // Not a config field: stored by name, and a config write cannot touch it. A
  // blank box therefore means "keep the stored one" rather than failing the
  // whole save, which is what used to force a retype on every unrelated edit.
  const handleCredential = (value: string) =>
    onChange({ secrets: { ...draft.secrets, gatewayAdminCredential: value } });
  const handleMetadata = (pairs: [string, string][]) => updateGateway({ identity_metadata: pairs });

  // The identity is read from the gateway, never typed. It is frozen at first
  // setup and decides which gateway an accepted allocation pays, so a typo
  // would be permanent — and the wizard never collected it at all, which made
  // first-time setup through the dashboard impossible: the daemon refused every
  // save for a field no screen offered.
  const probe = useProbeGateway();
  const canProbe =
    draft.gateway.admin_url.trim() !== '' &&
    draft.secrets.gatewayAdminCredential.trim() !== '' &&
    !probe.isPending;
  const handleProbe = () =>
    probe.mutate(draft, {
      onSuccess: (result) => updateGateway({ gateway_id: result.gateway_id })
    });

  return (
    <div className={styles.layout}>
      <TextInput
        label="Gateway name"
        value={draft.gateway.gateway_name}
        onChange={handleName}
        placeholder="my-gateway"
        error={errors.gateway_name}
      />

      <TextInput
        label="Admin URL"
        value={draft.gateway.admin_url}
        onChange={handleAdminUrl}
        placeholder="https://gateway.example.com"
        error={errors.admin_url}
      />

      <TextInput
        label="Admin credential"
        type="password"
        value={draft.secrets.gatewayAdminCredential}
        onChange={handleCredential}
        hint={credentialHint}
        error={errors.admin_credential}
      />

      <div className={styles.identity}>
        <Button
          variant="secondary"
          onClick={handleProbe}
          disabled={!canProbe}
          loading={probe.isPending}
        >
          {draft.gateway.gateway_id ? 'Check again' : 'Connect to gateway'}
        </Button>

        {probe.isError && (
          <Banner variant="error" title="Couldn't reach the gateway">
            {describeActionError(probe.error)}
          </Banner>
        )}

        {probe.data && (
          <Banner variant="success" title={`Connected to ${probe.data.lightning_alias}`}>
            Identity {probe.data.gateway_id} on {probe.data.network}. This is recorded once and
            cannot be changed afterwards.
          </Banner>
        )}

        {!probe.data && draft.gateway.gateway_id && (
          <p className={styles.identityNote}>Identity {draft.gateway.gateway_id}</p>
        )}
      </div>

      <FormField label="Identity metadata" hint={metadataHint}>
        {() => (
          <KeyValueEditor
            pairs={draft.gateway.identity_metadata}
            onChange={handleMetadata}
            keyPlaceholder="key"
            valuePlaceholder="value"
          />
        )}
      </FormField>
    </div>
  );
};
