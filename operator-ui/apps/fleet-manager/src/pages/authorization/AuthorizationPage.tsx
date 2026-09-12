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
        Your fleet needs to be approved before others can discover and use it. Scan the code below
        with the Holder app to approve it.
      </p>

      <AuthorizationPanel
        data={onboarding.data}
        isLoading={onboarding.isLoading}
        error={onboarding.error}
      />
      {holders.length > 0 ? (
        <SectionCard title="Approved by">
          <p className={styles.holdersHint}>
            Compare this ID with the one shown in the Holder app to confirm it matches.
          </p>

          <ul className={styles.holdersList}>{holders.map(renderHolder)}</ul>
        </SectionCard>
      ) : null}
    </div>
  );
};
