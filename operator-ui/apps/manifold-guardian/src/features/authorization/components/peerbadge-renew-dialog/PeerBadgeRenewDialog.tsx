import { Banner, Button } from '@operator-ui/common-ui';
import type { OnboardingResponse } from '@operator-ui/types';
import { type KeyboardEvent, type MouseEvent, useEffect, useId, useRef } from 'react';
import { PeerBadgeApprovalSteps } from '@/shared/components/peerbadge-approval-steps/PeerBadgeApprovalSteps';
import { describeActionError } from '@/shared/utils/describeActionError';
import { trapTab } from '@/shared/utils/trapTab';
import styles from './PeerBadgeRenewDialog.module.css';

interface PeerBadgeRenewDialogProps {
  data: OnboardingResponse;
  isFetching: boolean;
  error: unknown;
  onFetch: () => void;
  onClose: () => void;
}

export const PeerBadgeRenewDialog = ({
  data,
  isFetching,
  error,
  onFetch,
  onClose
}: PeerBadgeRenewDialogProps) => {
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const headingId = useId();

  useEffect(() => {
    const trigger = document.activeElement;
    closeButtonRef.current?.focus();
    return () => {
      if (trigger instanceof HTMLElement) trigger.focus();
    };
  }, []);

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    trapTab(event);
    if (event.key !== 'Escape') return;
    event.stopPropagation();
    onClose();
  };

  const handleBackdropClick = (event: MouseEvent<HTMLDivElement>) => {
    if (event.target === event.currentTarget) onClose();
  };

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby={headingId}
      onKeyDown={handleKeyDown}
      onClick={handleBackdropClick}
      className={styles.root}
    >
      <div className={styles.card}>
        <div className={styles.header}>
          <h2 id={headingId} className={styles.heading}>
            Renew your approval
          </h2>

          <Button ref={closeButtonRef} variant="secondary" size="small" onClick={onClose}>
            Close
          </Button>
        </div>

        <PeerBadgeApprovalSteps data={data} />

        {error ? <Banner variant="error">{describeActionError(error)}</Banner> : null}

        <div className={styles.actions}>
          <Button loading={isFetching} onClick={onFetch}>
            Fetch new authorization
          </Button>
        </div>
      </div>
    </div>
  );
};
