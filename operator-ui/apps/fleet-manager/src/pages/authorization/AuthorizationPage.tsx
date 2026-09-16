import { Button, SectionCard } from '@operator-ui/common-ui';
import { useAuthorizationWatch } from '@/shared/api/hooks/use-authorization-watch/useAuthorizationWatch';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { AuthorizationPanel } from '@/shared/components/authorization-panel/AuthorizationPanel';
import { GuardianTerms } from '@/shared/components/guardian-terms/GuardianTerms';
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
  const refresh = useAuthorizationWatch();
  const handleFetchAuthorization = () => {
    void refresh.refetch();
  };
  const nostr = onboarding.data?.nostr;
  const authorized = nostr?.state === 'authorization_observed';
  const holders = authorized ? nostr.holders : [];
  // An approved fleet is not waiting for a scan, so asking for one states
  // something the rest of the screen contradicts.
  const intro = authorized
    ? 'Your fleet is approved. The code below is your fleet manager ID.'
    : 'Your fleet needs to be approved before others can discover and use it. Scan the code below with the Holder app to approve it.';

  return (
    <div className={styles.root}>
      <h1 className={styles.heading}>Authorization</h1>

      <p className={styles.intro}>{intro}</p>

      <AuthorizationPanel
        data={onboarding.data}
        isLoading={onboarding.isLoading}
        error={refresh.error ?? onboarding.error}
      />

      <SectionCard title="Update authorization">
        <p className={styles.hint}>
          To renew or replace your authorization, scan the fleet manager ID with the Holder app and
          authorize it again. Then fetch the new authorization here. Your existing authorization is
          retained if the check fails or finds nothing new.
        </p>

        <Button variant="secondary" loading={refresh.isFetching} onClick={handleFetchAuthorization}>
          Fetch new authorization
        </Button>
      </SectionCard>
      {holders.length > 0 ? (
        <SectionCard title="Approved by">
          <p className={styles.hint}>
            Compare these with the approver shown in the Holder app to confirm they match.
          </p>

          <ul className={styles.holdersList}>{holders.map(renderHolder)}</ul>
        </SectionCard>
      ) : null}
      {/* No acceptance date: the daemon keeps no record of when, or whether, the
          terms were accepted, and a fleet onboarded before the terms step never
          saw it. */}
      <SectionCard title="Terms of service">
        <p className={styles.hint}>
          These terms cover Fedi verification and the telemetry your fleet shares with Fedi. Updates
          are posted at the same address.
        </p>

        <GuardianTerms />
      </SectionCard>
    </div>
  );
};
