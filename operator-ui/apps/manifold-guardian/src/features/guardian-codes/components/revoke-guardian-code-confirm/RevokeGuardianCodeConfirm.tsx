import { Button } from '@operator-ui/common-ui';
import type { SeatGrantEntry } from '@operator-ui/types';
import { type KeyboardEvent, useEffect, useId, useRef } from 'react';
import { trapTab } from '@/shared/utils/trapTab';
import styles from './RevokeGuardianCodeConfirm.module.css';

interface RevokeGuardianCodeConfirmProps {
  grant: SeatGrantEntry;
  onConfirm: () => void;
  onCancel: () => void;
  isPending: boolean;
  /** Why the last attempt failed, or null. */
  error: string | null;
}

export const RevokeGuardianCodeConfirm = ({
  grant,
  onConfirm,
  onCancel,
  isPending,
  error
}: RevokeGuardianCodeConfirmProps) => {
  // The safe choice takes focus: Enter must not revoke by accident.
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
          Revoke {grant.label ? `“${grant.label}”` : 'this guardian code'}?
        </h2>

        <div id={bodyId} className={styles.body}>
          <p>
            Nobody can use this code to add this guardian to a federation any more. A Federation
            Ambassador who tries it is told it can no longer be used.
          </p>

          <p>This can’t be undone. You can create a new code at any time.</p>
        </div>

        {error && (
          <p role="alert" className={styles.error}>
            {error}
          </p>
        )}

        <div className={styles.actions}>
          <Button variant="danger" loading={isPending} onClick={onConfirm}>
            Revoke code
          </Button>

          <Button ref={cancelButtonRef} variant="secondary" disabled={isPending} onClick={onCancel}>
            Keep code
          </Button>
        </div>
      </div>
    </div>
  );
};
