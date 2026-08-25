import { Button, TextInput } from '@operator-ui/common-ui';
import { type KeyboardEvent, useEffect, useId, useRef, useState } from 'react';
import styles from './WithdrawConfirm.module.css';

interface WithdrawConfirmProps {
  onConfirm: (reason: string | null) => void;
  onCancel: () => void;
  isPending: boolean;
}

export const WithdrawConfirm = ({ onConfirm, onCancel, isPending }: WithdrawConfirmProps) => {
  const [reason, setReason] = useState('');
  const panelRef = useRef<HTMLDivElement>(null);
  const labelId = useId();

  // Opening this panel unmounts the button that was focused. Without taking
  // focus, a keyboard or screen-reader operator is dropped back to the top of
  // the document with no announcement that a confirmation is now waiting — so
  // the panel claims focus and names itself on arrival.
  useEffect(() => {
    panelRef.current?.focus();
  }, []);

  const handleConfirm = () => {
    const trimmed = reason.trim();
    onConfirm(trimmed.length > 0 ? trimmed : null);
  };

  // Escape is the expected way out of a confirmation, and it is the only exit a
  // keyboard operator can reach without tabbing through the reason field.
  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Escape' || isPending) return;
    event.stopPropagation();
    onCancel();
  };

  return (
    // This is a confirmation prompt, not a set of related form fields, so
    // <fieldset>/<legend> would misname it — and the legend is excluded from
    // the panel's flex layout besides. tabIndex -1 is programmatic focus for
    // the prompt, not a tab stop: tabbing goes straight to the reason field
    // and the two actions.
    // biome-ignore lint/a11y/useSemanticElements: see the note above
    <div
      ref={panelRef}
      role="group"
      aria-labelledby={labelId}
      tabIndex={-1}
      onKeyDown={handleKeyDown}
      className={styles.panel}
    >
      <span id={labelId} className={styles.label}>
        Withdraw this advertisement?
      </span>

      <TextInput
        label="Reason (optional)"
        value={reason}
        onChange={setReason}
        disabled={isPending}
      />

      <div className={styles.actions}>
        <Button variant="danger" size="small" loading={isPending} onClick={handleConfirm}>
          Confirm withdrawal
        </Button>

        <Button variant="secondary" size="small" disabled={isPending} onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </div>
  );
};
