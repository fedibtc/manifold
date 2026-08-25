import { Banner, Button } from '@operator-ui/common-ui';
import type { SafeRestoreError } from '@/features/setup/utils/restoreViewState';
import styles from './SetupRestoreFailed.module.css';

interface SetupRestoreFailedProps {
  error: SafeRestoreError;
  onTryAgain: () => void;
  onBackToDoors: () => void;
}

// The message is shown, not interpreted. The daemon's restore errors already name
// the cause and the action; re-deriving that here would mean matching prose, which
// `BE-FMAN-RECOVERY-003` exists to make unnecessary.
//
// The screen claims only that no identity was installed, which `install` guarantees
// by writing the identity last. It does not claim the disk is untouched: per
// `crates/fman/core/src/restore.rs`, a refusal part-way through can leave a seat
// directory behind, and the next attempt then refuses that directory.
export const SetupRestoreFailed = ({
  error,
  onTryAgain,
  onBackToDoors
}: SetupRestoreFailedProps) => (
  <div className={styles.root}>
    <div className={styles.head}>
      <h1 className={styles.heading}>Recovery did not complete</h1>

      <p className={styles.intro}>
        The fleet manager refused the request, so this host still has no identity. Read what it said
        before you retry — a refusal part-way through can leave a seat directory behind.
      </p>
    </div>

    <Banner variant="error">The fleet manager said:</Banner>

    <p className={styles.message}>{error.message}</p>

    <div className={styles.actions}>
      <Button variant="secondary" onClick={onBackToDoors}>
        Back to setup options
      </Button>

      <Button onClick={onTryAgain}>Try again</Button>
    </div>
  </div>
);
