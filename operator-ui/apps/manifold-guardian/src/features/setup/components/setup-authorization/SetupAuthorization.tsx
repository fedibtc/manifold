import { Button } from '@operator-ui/common-ui';
import { useEffect, useRef } from 'react';
import { CheckStatusButton } from '@/features/setup/components/check-status-button/CheckStatusButton';
import { PeerBadgeHelp } from '@/features/setup/components/peerbadge-help/PeerBadgeHelp';
import { SetupAuthorizationPanel } from '@/features/setup/components/setup-authorization-panel/SetupAuthorizationPanel';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { useRefreshAuthorization } from '@/shared/api/hooks/use-refresh-authorization/useRefreshAuthorization';
import { AuthorizationStatusBanner } from '@/shared/components/authorization-status-banner/AuthorizationStatusBanner';
import { isAuthorized } from '@/shared/utils/authorization';
import styles from './SetupAuthorization.module.css';

const CONTINUE_DELAY_MS = 2_000;

interface SetupAuthorizationProps {
  onSettled: () => void;
}

export const SetupAuthorization = ({ onSettled }: SetupAuthorizationProps) => {
  const onboarding = useOnboarding();
  const refresh = useRefreshAuthorization();
  const authorized = isAuthorized(onboarding.data);
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

      <div className={styles.actions}>
        <Button disabled>Continue</Button>
      </div>
    </div>
  );
};
