import { SectionCard } from '@operator-ui/common-ui';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { AuthorizationPanel } from '@/shared/components/authorization-panel/AuthorizationPanel';
import { toNpub } from '@/shared/utils/npub';
import styles from './AuthorizationPage.module.css';

// Keyed by the reported value, which is unique per holder; the npub is a
// rendering of it and an unencodable key still needs a stable key.
const renderHolder = (holder: string) => (
  <li key={holder} className={styles.holder}>
    {toNpub(holder) ?? holder}
  </li>
);

export const AuthorizationPage = () => {
  const onboarding = useOnboarding();
  const nostr = onboarding.data?.nostr;
  const authorized = nostr?.state === 'authorization_observed';
  const holders = authorized ? nostr.holders : [];

  return (
    <div className={styles.root}>
      <h1 className={styles.heading}>Authorization</h1>

      <p className={styles.intro}>
        Until a holder has authorized this fleet manager, initiators have no way to evaluate it.
        This page stays available for as long as the fleet runs.
      </p>

      <AuthorizationPanel
        data={onboarding.data}
        isLoading={onboarding.isLoading}
        error={onboarding.error}
      />
      {holders.length > 0 ? (
        <SectionCard title="Observed holders">
          <p className={styles.holdersHint}>
            Shown as an npub, so this can be compared against the identity key a holder application
            displays.
          </p>

          <ul className={styles.holdersList}>{holders.map(renderHolder)}</ul>
        </SectionCard>
      ) : null}
    </div>
  );
};
