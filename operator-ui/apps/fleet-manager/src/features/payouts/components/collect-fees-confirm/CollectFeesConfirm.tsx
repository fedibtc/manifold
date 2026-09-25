import { Button } from '@operator-ui/common-ui';
import { type KeyboardEvent, useEffect, useId, useRef } from 'react';
import { formatSats } from '@/shared/utils/format';
import styles from './CollectFeesConfirm.module.css';

interface CollectFeesConfirmProps {
  collectableMsat: number;
  onConfirm: () => void;
  onCancel: () => void;
  isPending: boolean;
}

/**
 * Known gap, shared with `UpdateRequiredTakeover`: focus moves here and Escape
 * closes it, but nothing traps Tab inside. There is no dialog primitive in this
 * repository to inherit that from.
 */
export const CollectFeesConfirm = ({
  collectableMsat,
  onConfirm,
  onCancel,
  isPending
}: CollectFeesConfirmProps) => {
  const confirmButtonRef = useRef<HTMLButtonElement>(null);
  const headingId = useId();
  const bodyId = useId();

  useEffect(() => {
    confirmButtonRef.current?.focus();
  }, []);

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
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
          Collect {formatSats(collectableMsat)} now?
        </h2>

        <p id={bodyId} className={styles.body}>
          Minting the ecash costs a fee for every note it creates, approximately 0.1 sat each. You
          will receive less than the pool shows. A small collection loses proportionally more of
          itself, so letting the pool grow keeps more of it.
        </p>

        <div className={styles.actions}>
          <Button ref={confirmButtonRef} loading={isPending} onClick={onConfirm}>
            Collect anyway
          </Button>

          <Button variant="secondary" disabled={isPending} onClick={onCancel}>
            Cancel
          </Button>
        </div>
      </div>
    </div>
  );
};
