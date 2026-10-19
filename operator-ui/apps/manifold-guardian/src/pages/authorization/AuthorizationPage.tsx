import { Button, SectionCard } from '@operator-ui/common-ui';
import { useState } from 'react';
import { PeerBadgeRenewDialog } from '@/features/authorization/components/peerbadge-renew-dialog/PeerBadgeRenewDialog';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { useRefreshAuthorization } from '@/shared/api/hooks/use-refresh-authorization/useRefreshAuthorization';
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
  const refresh = useRefreshAuthorization();
  const [isGuideOpen, setIsGuideOpen] = useState(false);
  const handleFetchAuthorization = () => {
    refresh.mutate();
  };
  const handleOpenGuide = () => {
    setIsGuideOpen(true);
  };
  const handleCloseGuide = () => {
    setIsGuideOpen(false);
  };
  const handleFetchFromGuide = () => {
    refresh.mutate(undefined, { onSuccess: handleCloseGuide });
  };
  const nostr = onboarding.data?.nostr;
  const authorized = nostr?.state === 'authorization_observed';
  const holders = authorized ? nostr.holders : [];
  // An approved fleet is not waiting for a scan, so asking for one states
  // something the rest of the screen contradicts.
  const intro = authorized
    ? 'This host is approved. The code below is your Manifold Fedimint Guardian ID.'
    : 'This host needs to be approved before Federation Ambassadors can discover and use it. Scan the code below with the PeerBadge app to approve it.';
  const guideTitle = authorized ? 'Renew your approval' : 'Approve your host';

  const showMeHow = (
    <Button variant="secondary" onClick={handleOpenGuide}>
      Show me how
    </Button>
  );

  return (
    <div className={styles.root}>
      <h1 className={styles.heading}>Authorization</h1>

      <p className={styles.intro}>{intro}</p>

      <AuthorizationPanel
        data={onboarding.data}
        isLoading={onboarding.isLoading}
        error={refresh.error ?? onboarding.error}
      />
      {onboarding.data && !authorized ? <div className={styles.actions}>{showMeHow}</div> : null}

      <SectionCard title="Update authorization">
        <p className={styles.hint}>
          To renew, approve this host again in the PeerBadge app. Then fetch the new authorization
          here. Your existing authorization is kept if the check fails or finds nothing new.
        </p>

        <div className={styles.actions}>
          <Button
            variant="secondary"
            loading={refresh.isPending}
            onClick={handleFetchAuthorization}
          >
            Fetch new authorization
          </Button>

          {authorized ? showMeHow : null}
        </div>
      </SectionCard>
      {holders.length > 0 ? (
        <SectionCard title="Approved by">
          <p className={styles.hint}>
            Compare these with the approver shown in the PeerBadge app to confirm they match.
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
      {isGuideOpen && onboarding.data ? (
        <PeerBadgeRenewDialog
          title={guideTitle}
          data={onboarding.data}
          isFetching={refresh.isPending}
          error={refresh.error}
          onFetch={handleFetchFromGuide}
          onClose={handleCloseGuide}
        />
      ) : null}
    </div>
  );
};
