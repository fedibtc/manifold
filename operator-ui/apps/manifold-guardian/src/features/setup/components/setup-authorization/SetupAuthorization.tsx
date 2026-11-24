import { Banner, Button } from '@operator-ui/common-ui';
import { useEffect, useRef } from 'react';
import { useSkipHolderAuthorization } from '@/features/setup/api/hooks/use-skip-holder-authorization/useSkipHolderAuthorization';
import { PeerBadgeHelp } from '@/features/setup/components/peerbadge-help/PeerBadgeHelp';
import { SetupAuthorizationPanel } from '@/features/setup/components/setup-authorization-panel/SetupAuthorizationPanel';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { useRefreshAuthorization } from '@/shared/api/hooks/use-refresh-authorization/useRefreshAuthorization';
import { AuthorizationStatusBanner } from '@/shared/components/authorization-status-banner/AuthorizationStatusBanner';
import { isAuthorized } from '@/shared/utils/authorization';
import { describeActionError } from '@/shared/utils/describeActionError';
import styles from './SetupAuthorization.module.css';

const CONTINUE_DELAY_MS = 2_000;

interface SetupAuthorizationProps {
  onSettled: () => void;
}

export const SetupAuthorization = ({ onSettled }: SetupAuthorizationProps) => {
  const onboarding = useOnboarding();
  const refresh = useRefreshAuthorization();
  const skip = useSkipHolderAuthorization();
  const authorized = isAuthorized(onboarding.data);
  // Off in production until bring-your-own guardians ships.
  const badgeless = onboarding.data?.badgeless_mg === true;
  // One guard for the timer and the manual continue button landing together.
  const hasSettled = useRef(false);

  const settleOnce = () => {
    if (hasSettled.current) return;
    hasSettled.current = true;
    onSettled();
  };

  // Relay reconciliation is explicit: setup performs no background refreshes.
  const handleCheckNow = () => {
    refresh.mutate();
  };

  // Without a badge the daemon settles the stage at once; there is nothing to
  // show before moving on.
  const handleSkip = () => {
    skip.mutate(undefined, { onSuccess: settleOnce });
  };

  useEffect(() => {
    if (!authorized) return;

    const timer = setTimeout(() => {
      if (hasSettled.current) return;
      hasSettled.current = true;
      onSettled();
    }, CONTINUE_DELAY_MS);

    return () => clearTimeout(timer);
  }, [authorized, onSettled]);

  if (authorized && onboarding.data) {
    return (
      <div className={styles.root}>
        <div className={styles.head}>
          <h1 className={styles.heading}>Approve your host</h1>
        </div>

        <AuthorizationStatusBanner nostr={onboarding.data.nostr} />

        <p className={styles.statusLine} role="status">
          <span className={styles.spinner} aria-hidden="true" />
          Approved. Continuing to the terms step…
        </p>

        <div className={styles.actions}>
          <Button onClick={settleOnce}>Continue now</Button>
        </div>
      </div>
    );
  }

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <h1 className={styles.heading}>Approve your host</h1>

        <p className={styles.intro}>
          Use the PeerBadge app on your phone. You need a Trusted badge from a Fedi signer,
          typically your local Fedi Master or Knight.
        </p>
      </div>

      <SetupAuthorizationPanel
        data={onboarding.data}
        isLoading={onboarding.isLoading}
        error={refresh.error ?? onboarding.error}
      />

      <PeerBadgeHelp />

      {/* One primary action: the badge path. Approval continues on its own, so
          there is no Continue to grey out beside it. */}
      <div className={styles.actions}>
        <Button loading={refresh.isPending} onClick={handleCheckNow}>
          Check approval
        </Button>
      </div>

      {/* The badgeless route sits below the badge steps and stays quiet: most
          hosts should take the badge path. */}
      {badgeless ? (
        <section className={styles.alternative} aria-labelledby="setup-without-badge">
          <h2 id="setup-without-badge" className={styles.alternativeTitle}>
            No Trusted badge?
          </h2>

          <p className={styles.hint}>
            Hosting for yourself or people you know? Set this host up without a badge. Federation
            Ambassadors you give a guardian code to can still add it to their federation, free. It
            won’t appear in Fedi-verified selection, so there is no seat price to set. You can add a
            badge later on the Guardian badge page.
          </p>

          <div className={styles.actions}>
            <Button variant="secondary" size="small" loading={skip.isPending} onClick={handleSkip}>
              Set up without a badge
            </Button>
          </div>

          {skip.error ? <Banner variant="error">{describeActionError(skip.error)}</Banner> : null}
        </section>
      ) : null}
    </div>
  );
};
