import { Banner, Button, SectionCard } from '@operator-ui/common-ui';
import { AttestationPanel } from '@/features/attestations/components/attestation-panel/AttestationPanel';
import { BackupCard } from '@/features/settings/components/backup-card/BackupCard';
import { useProviderConfigForm } from '@/features/settings/hooks/use-provider-config-form/useProviderConfigForm';
import { ChainObserverStep } from '@/features/setup/components/steps/chain-observer-step/ChainObserverStep';
import { GatewayStep } from '@/features/setup/components/steps/gateway-step/GatewayStep';
import { NetworkStep } from '@/features/setup/components/steps/network-step/NetworkStep';
import { PolicyCapacityStep } from '@/features/setup/components/steps/policy-capacity-step/PolicyCapacityStep';
import { RelaysEndpointStep } from '@/features/setup/components/steps/relays-endpoint-step/RelaysEndpointStep';
import { validateRelaysEndpoint } from '@/features/setup/services/validation';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import styles from './SettingsPage.module.css';

const NO_ERRORS = {};

const credentialNotice =
  'The gateway admin credential and the chain-observer password are write-only and are never sent back. Leave either blank to keep the stored one; changing one takes effect on its own, without rewriting the rest of this page.';

const PageHeader = () => (
  <header className={styles.head}>
    <h1 className={styles.heading}>Settings</h1>

    <p className={styles.sub}>Review and edit your provider configuration.</p>
  </header>
);

export const SettingsPage = () => {
  const form = useProviderConfigForm();

  // Three facts the screen used to collapse into one: nothing has answered yet,
  // the last attempt failed, and we hold an older answer. The surface keeps
  // them apart, so a failed refresh over a good config now leaves the form on
  // screen under a staleness marker instead of blanking it.
  if (form.status !== 'ready') {
    return (
      <div className={styles.page}>
        <PageHeader />

        <QuerySurface disposition={form.disposition} onRetry={form.retry}>
          {null}
        </QuerySurface>
      </div>
    );
  }

  const { draft, onChange, save, isSaving, phase, saveError, failedChecks } = form;

  return (
    <div className={styles.page}>
      <PageHeader />

      <QuerySurface disposition={form.disposition} onRetry={form.retry}>
        <SectionCard title="Policy & capacity">
          <PolicyCapacityStep draft={draft} onChange={onChange} errors={NO_ERRORS} />
        </SectionCard>

        <SectionCard title="Relays & endpoint">
          {/*
          The wizard's own validator, reused. Settings mounts the same steps and
          used to mount every one of them with no validator at all, so a value
          the wizard refuses — a republish interval of zero, say — could still be
          saved from here. The daemon refuses it now either way; this reports it
          at the field instead of as a failed check after the round trip.
        */}
          <RelaysEndpointStep
            draft={draft}
            onChange={onChange}
            errors={validateRelaysEndpoint(draft)}
          />
        </SectionCard>

        <SectionCard title="Network">
          <NetworkStep draft={draft} onChange={onChange} errors={NO_ERRORS} />
        </SectionCard>

        <SectionCard title="Gateway">
          <Banner variant="info" title="Secrets are kept separately">
            {credentialNotice}
          </Banner>

          <GatewayStep draft={draft} onChange={onChange} errors={NO_ERRORS} />
        </SectionCard>

        <SectionCard title="Chain observer">
          <ChainObserverStep draft={draft} onChange={onChange} errors={NO_ERRORS} />
        </SectionCard>

        <BackupCard />

        <SectionCard title="Attestations">
          <AttestationPanel />
        </SectionCard>

        {phase === 'validation_failed' && failedChecks.length > 0 ? (
          <Banner variant="error" title={`Couldn't save — ${failedChecks.length} checks failed`}>
            <ul className={styles.checkList}>
              {failedChecks.map((check) => (
                <li key={check.name}>
                  {check.name}
                  {check.detail ? ` — ${check.detail}` : ''}
                </li>
              ))}
            </ul>
          </Banner>
        ) : null}

        {saveError ? (
          <Banner variant="error" title="Couldn't save">
            {saveError}
          </Banner>
        ) : null}

        <div className={styles.actions}>
          <Button variant="primary" onClick={save} loading={isSaving}>
            Save changes
          </Button>
        </div>
      </QuerySurface>
    </div>
  );
};
