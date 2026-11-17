import { Button } from '@operator-ui/common-ui';
import { type KeyboardEvent, useEffect, useId, useRef } from 'react';
import { trapTab } from '@/shared/utils/trapTab';
import styles from './ResetUptimeConfirm.module.css';

interface ResetUptimeConfirmProps {
  onConfirm: () => void;
  onCancel: () => void;
  isPending: boolean;
  /** Why the last attempt failed, or null. */
  error: string | null;
}

export const ResetUptimeConfirm = ({
  onConfirm,
  onCancel,
  isPending,
  error
}: ResetUptimeConfirmProps) => {
  // The safe choice takes focus: Enter must not reset by accident.
  const cancelButtonRef = useRef<HTMLButtonElement>(null);
  const headingId = useId();
  const bodyId = useId();

  useEffect(() => {
    cancelButtonRef.current?.focus();
  }, []);

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    trapTab(event);
    if (event.key !== 'Escape' || isPending) return;
    event.stopPropagation();
    onCancel();
  };

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby={headingId}
      aria-describedby={bodyId}
      onKeyDown={handleKeyDown}
      className={styles.root}
    >
      <div className={styles.card}>
        <h2 id={headingId} className={styles.heading}>
          Reset uptime?
        </h2>

        <div id={bodyId} className={styles.body}>
          <p>
            <strong>Only do this if your badge issuer asked you to.</strong> Resetting uptime
            without their say-so can get your badge revoked.
          </p>

          <p>
            Your uptime history restarts at 95%, as on a new host, and the readiness checks run
            again now.
          </p>
        </div>

        {error && (
          <p role="alert" className={styles.error}>
            {error}
          </p>
        )}

        <div className={styles.actions}>
          <Button variant="danger" loading={isPending} onClick={onConfirm}>
            Reset uptime
          </Button>

          <Button ref={cancelButtonRef} variant="secondary" disabled={isPending} onClick={onCancel}>
            Cancel
          </Button>
        </div>
      </div>
    </div>
  );
};
