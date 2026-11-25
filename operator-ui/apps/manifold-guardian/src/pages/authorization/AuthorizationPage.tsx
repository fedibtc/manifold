import { Button, SectionCard } from '@operator-ui/common-ui';
import { useState } from 'react';
import { PeerBadgeRenewDialog } from '@/features/authorization/components/peerbadge-renew-dialog/PeerBadgeRenewDialog';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { useRefreshAuthorization } from '@/shared/api/hooks/use-refresh-authorization/useRefreshAuthorization';
import { AuthorizationPanel } from '@/shared/components/authorization-panel/AuthorizationPanel';
import { GuardianTerms } from '@/shared/components/guardian-terms/GuardianTerms';
import { guardianCodesAvailable, isBadgeless } from '@/shared/utils/authorization';
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
  const badgeless = isBadgeless(onboarding.data);
  // An approved fleet is not waiting for a scan, so asking for one states
  // something the rest of the screen contradicts. A host without a badge is
  // not waiting either: guardian codes already work, and a badge only adds
  // Fedi-verified selection.
  // Guardian codes are mentioned only where the network offers them.
  const alsoCodes = guardianCodesAvailable(onboarding.data) ? ' Guardian codes work as well.' : '';
  const intro = authorized
    ? `This host has a Trusted badge, so Federation Ambassadors can find and select it for Fedi-verified federations.${alsoCodes} The code below is your Manifold Fedimint Guardian ID.`
    : badgeless
      ? 'This host runs without a badge and takes seats through guardian codes. Add a Trusted badge from a Fedi signer to also let Federation Ambassadors find and select it for Fedi-verified federations, and to set a seat price.'
      : 'This host needs to be approved before Federation Ambassadors can discover and use it. Scan the code below with the PeerBadge app to approve it.';
  const guideTitle = authorized
    ? 'Renew your approval'
    : badgeless
      ? 'Add a badge'
      : 'Approve your host';

  const showMeHow = (
    <Button variant="secondary" onClick={handleOpenGuide}>
      Show me how
    </Button>
  );

  return (
    <div className={styles.root}>
      <h1 className={styles.heading}>Guardian badge</h1>

      <p className={styles.intro}>{intro}</p>

      <AuthorizationPanel
        data={onboarding.data}
        isLoading={onboarding.isLoading}
        error={refresh.error ?? onboarding.error}
        badgeless={badgeless}
      />
      {onboarding.data && badgeless ? (
        <div className={styles.actions}>
          <Button
            variant="secondary"
            loading={refresh.isPending}
            onClick={handleFetchAuthorization}
          >
            Check for my badge
          </Button>

          {showMeHow}
        </div>
      ) : null}
      {onboarding.data && !authorized && !badgeless ? (
        <div className={styles.actions}>{showMeHow}</div>
      ) : null}
      {badgeless ? null : (
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
      )}
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
