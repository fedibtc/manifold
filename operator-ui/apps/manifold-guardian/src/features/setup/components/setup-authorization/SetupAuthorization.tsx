import { Banner, Button } from '@operator-ui/common-ui';
import { useEffect, useRef } from 'react';
import { useSkipHolderAuthorization } from '@/features/setup/api/hooks/use-skip-holder-authorization/useSkipHolderAuthorization';
import { CheckStatusButton } from '@/features/setup/components/check-status-button/CheckStatusButton';
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

  const checkStatus = <CheckStatusButton loading={refresh.isPending} onClick={handleCheckNow} />;

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
        statusAction={checkStatus}
      />

      <PeerBadgeHelp />

      {badgeless ? (
        <p className={styles.hint}>
          No badge? You can set this host up without one. Federation Ambassadors can’t find it until
          you approve it later on the Authorization page.
        </p>
      ) : null}

      <div className={styles.actions}>
        <Button disabled>Continue</Button>

        {badgeless ? (
          <Button variant="secondary" loading={skip.isPending} onClick={handleSkip}>
            Continue without a badge
          </Button>
        ) : null}
      </div>

      {skip.error ? <Banner variant="error">{describeActionError(skip.error)}</Banner> : null}
    </div>
  );
};
