import { Banner, Button } from '@operator-ui/common-ui';
import type { SafeRestoreError } from '@/features/setup/utils/restoreViewState';
import styles from './SetupRestoreUnknown.module.css';

interface SetupRestoreUnknownProps {
  error: SafeRestoreError;
  isChecking: boolean;
  identityConfirmed: boolean;
  onCheckStatus: () => void;
  onContinue: () => void;
}

// A lost response is not a failure. The daemon may have installed the identity
// before the browser lost the answer, so this screen offers a status check and
// never another restore. `BE-FMAN-RECOVERY-002` replaces the inference with an
// explicit operation result.
export const SetupRestoreUnknown = ({
  error,
  isChecking,
  identityConfirmed,
  onCheckStatus,
  onContinue
}: SetupRestoreUnknownProps) => (
  <div className={styles.root}>
    <div className={styles.head}>
      <h1 className={styles.heading}>Recovery result unknown</h1>

      <p className={styles.intro}>
        The connection dropped before the fleet manager answered, so we do not know whether the
        recovery finished. A second attempt could not be undone, so this screen checks instead.
      </p>
    </div>

    <Banner variant="warn">{error.message}</Banner>
    {identityConfirmed ? (
      <p className={styles.detail}>
        This host has an identity, so the recovery did complete. The recovery counts are not
        available, because the answer that carried them was lost.
      </p>
    ) : null}

    <div className={styles.actions}>
      {identityConfirmed ? (
        <Button onClick={onContinue}>Continue</Button>
      ) : (
        <Button disabled={isChecking} loading={isChecking} onClick={onCheckStatus}>
          Check status
        </Button>
      )}
    </div>
  </div>
);
