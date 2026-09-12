import { Button } from '@operator-ui/common-ui';
import { useEffect, useRef } from 'react';
import { useAuthorizationWatch } from '@/shared/api/hooks/use-authorization-watch/useAuthorizationWatch';
import { AuthorizationPanel } from '@/shared/components/authorization-panel/AuthorizationPanel';
import { isAuthorized } from '@/shared/utils/authorization';
import styles from './SetupAuthorization.module.css';

const CONTINUE_DELAY_MS = 2_000;

interface SetupAuthorizationProps {
  onSettled: () => void;
}

export const SetupAuthorization = ({ onSettled }: SetupAuthorizationProps) => {
  const onboarding = useAuthorizationWatch();
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
    void onboarding.refetch();
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

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <h1 className={styles.heading}>Get your fleet approved</h1>

        {/* This, not the standalone Authorization page, is what an unapproved
            operator actually sees: SetupGate holds the whole app here until the
            daemon reports onboarding complete. The consequences of staying
            unapproved therefore have to be stated here. */}
        <p className={styles.intro}>
          Your fleet needs to be approved before others can discover and use it. Scan the code below
          with the Holder app. Until it is approved your fleet is not advertised and cannot sell
          seats — setup cannot continue past this step.
        </p>
      </div>

      <AuthorizationPanel
        data={onboarding.data}
        isLoading={onboarding.isLoading}
        error={onboarding.error}
      />
      {authorized ? (
        <p className={styles.statusLine} role="status">
          <span className={styles.spinner} aria-hidden="true" />
          Approved. Continuing to the price step…
        </p>
      ) : null}

      <div className={styles.actions}>
        {authorized ? null : (
          <Button variant="secondary" loading={onboarding.isFetching} onClick={handleCheckNow}>
            Check now
          </Button>
        )}
        {authorized ? (
          <Button onClick={settleOnce}>Continue now</Button>
        ) : (
          <Button disabled>Continue</Button>
        )}
      </div>
    </div>
  );
};
