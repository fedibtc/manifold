import { Button, TextInput } from '@operator-ui/common-ui';
import { type FormEvent, useState } from 'react';
import { useSetPayoutDestination } from '@/features/payouts/api/hooks/use-set-payout-destination/useSetPayoutDestination';
import { DialogRows } from '@/features/payouts/components/dialog-rows/DialogRows';
import { PayoutDialog } from '@/features/payouts/components/payout-dialog/PayoutDialog';
import {
  isPayoutDestinationFormat,
  normalizePayoutDestination
} from '@/features/payouts/utils/payoutDestination';
import { describeActionError } from '@/shared/utils/describeActionError';
import styles from './PayoutAddressDialog.module.css';

interface PayoutAddressDialogProps {
  /** The stored address, or null when none is saved. */
  destination: string | null;
  onClose: () => void;
  /** The address was saved or removed. */
  onDone: () => void;
}

const NOT_A_DESTINATION = 'Enter a Lightning address (name@example.com) or an LNURL (lnurl1…).';

type Step = { kind: 'edit' } | { kind: 'confirm-change'; to: string } | { kind: 'confirm-remove' };

/**
 * Adds, changes, or removes the Lightning payout address. A change and a
 * removal are confirmed from → to first: every later Lightning withdrawal goes
 * where this points.
 */
export const PayoutAddressDialog = ({ destination, onClose, onDone }: PayoutAddressDialogProps) => {
  const save = useSetPayoutDestination();
  const [value, setValue] = useState(destination ?? '');
  const [formatError, setFormatError] = useState<string | null>(null);
  const [step, setStep] = useState<Step>({ kind: 'edit' });
  const normalized = normalizePayoutDestination(value);

  const store = (next: string | null) => {
    save.mutate(next, { onSuccess: onDone, onError: () => setStep({ kind: 'edit' }) });
  };

  const handleSave = () => {
    if (normalized === '') return;
    if (!isPayoutDestinationFormat(normalized)) {
      setFormatError(NOT_A_DESTINATION);
      return;
    }
    if (destination === null) {
      store(normalized);
    } else if (normalized === destination) {
      onClose();
    } else {
      setStep({ kind: 'confirm-change', to: normalized });
    }
  };

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    handleSave();
  };

  const error = formatError ?? (save.isError ? describeActionError(save.error) : null);
  const title = destination === null ? 'Add payout address' : 'Change payout address';

  if (step.kind === 'confirm-change' && destination !== null) {
    return (
      <PayoutDialog
        title="Change payout address?"
        onDismiss={() => setStep({ kind: 'edit' })}
        note="Every later Lightning withdrawal goes to the new address."
        footer={
          <>
            <Button
              variant="secondary"
              disabled={save.isPending}
              onClick={() => setStep({ kind: 'edit' })}
            >
              Back
            </Button>

            <Button loading={save.isPending} onClick={() => store(step.to)}>
              Change address
            </Button>
          </>
        }
      >
        <DialogRows
          rows={[
            { label: 'From', value: destination },
            { label: 'To', value: step.to }
          ]}
        />
      </PayoutDialog>
    );
  }

  if (step.kind === 'confirm-remove' && destination !== null) {
    return (
      <PayoutDialog
        title="Remove payout address?"
        onDismiss={() => setStep({ kind: 'edit' })}
        note="Withdrawals over Lightning stop until you add an address again. Ecash works without it."
        footer={
          <>
            <Button
              variant="secondary"
              disabled={save.isPending}
              onClick={() => setStep({ kind: 'edit' })}
            >
              Back
            </Button>

            <Button variant="danger" loading={save.isPending} onClick={() => store(null)}>
              Remove address
            </Button>
          </>
        }
      >
        <DialogRows rows={[{ label: 'Address', value: destination }]} />
      </PayoutDialog>
    );
  }

  return (
    <PayoutDialog
      title={title}
      onDismiss={onClose}
      footerStart={
        destination !== null && (
          <button
            type="button"
            className={styles.dangerLink}
            onClick={() => setStep({ kind: 'confirm-remove' })}
          >
            Remove address
          </button>
        )
      }
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>

          <Button disabled={normalized === ''} loading={save.isPending} onClick={handleSave}>
            Save address
          </Button>
        </>
      }
    >
      <form className={styles.form} onSubmit={handleSubmit}>
        <TextInput
          label="Lightning address or LNURL-pay"
          value={value}
          onChange={(next) => {
            setValue(next);
            setFormatError(null);
          }}
          disabled={save.isPending}
          placeholder="operator@example.com"
          hint="A reusable Lightning address, like operator@example.com. One-time invoices won't work: every withdrawal uses this address."
          error={error ?? undefined}
        />
      </form>
    </PayoutDialog>
  );
};
