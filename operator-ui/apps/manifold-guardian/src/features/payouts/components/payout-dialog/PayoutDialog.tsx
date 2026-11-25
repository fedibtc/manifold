import { CopyButton, isTruncated, truncateMiddle } from '@operator-ui/common-ui';
import { type KeyboardEvent, type ReactNode, useEffect, useId, useRef } from 'react';
import { type PayoutSource, sourceId } from '@/features/payouts/utils/payoutSource';
import { trapTab } from '@/shared/utils/trapTab';
import styles from './PayoutDialog.module.css';

interface PayoutDialogProps {
  title: string;
  /** The balance this withdrawal draws on; its full ID shows under the title,
   *  copyable. Absent for the payout address, which belongs to no balance. */
  source?: PayoutSource;
  /** The ecash code step and its cancel confirmation need two columns. */
  wide?: boolean;
  /** Escape closes the dialog through this. Absent while a money move is
   *  committing, so the step cannot be dismissed. */
  onDismiss?: () => void;
  /** A grey note under the body: what the operator should know before acting. */
  note?: ReactNode;
  /** A destructive link at the footer's far left, away from the buttons. */
  footerStart?: ReactNode;
  footer: ReactNode;
  children: ReactNode;
}

/**
 * The one dialog every withdrawal step renders in: review, progress, result,
 * the ecash code and its cancel. Steps swap the body inside it rather than
 * stacking a second dialog.
 */
export const PayoutDialog = ({
  title,
  source,
  wide = false,
  onDismiss,
  note,
  footerStart,
  footer,
  children
}: PayoutDialogProps) => {
  const cardRef = useRef<HTMLDivElement>(null);
  const titleId = useId();

  useEffect(() => {
    cardRef.current?.focus();
  }, []);

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    trapTab(event);
    if (event.key !== 'Escape') return;
    event.stopPropagation();
    onDismiss?.();
  };

  return (
    <div className={styles.overlay}>
      <div
        ref={cardRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        onKeyDown={handleKeyDown}
        className={styles.card}
        data-wide={wide}
      >
        <div className={styles.head}>
          <h2 id={titleId} className={styles.title}>
            {title}
          </h2>

          {source && <SourceId source={source} />}
        </div>

        {children}

        {note && <p className={styles.note}>{note}</p>}

        <div className={styles.footer}>
          {footerStart && <span className={styles.footerStart}>{footerStart}</span>}
          {footer}
        </div>
      </div>
    </div>
  );
};

const SourceId = ({ source }: { source: PayoutSource }) => {
  const id = sourceId(source);
  return (
    <span className={styles.idRow}>
      <span className={styles.mono}>{truncateMiddle(id, 8, 8)}</span>
      {isTruncated(id, 8, 8) && (
        <CopyButton
          value={id}
          label={source.kind === 'payment_federation' ? 'Copy federation ID' : 'Copy seat ID'}
        />
      )}
    </span>
  );
};
