import { Button, CopyButton, truncateMiddle } from '@operator-ui/common-ui';
import { useId } from 'react';
import { useSweepPaymentFees } from '@/features/payouts/api/hooks/use-sweep-payment-fees/useSweepPaymentFees';
import { describePayout } from '@/features/payouts/utils/sweepOutcome';
import { describeActionError } from '@/shared/utils/describeActionError';
import styles from './PaymentSweepAction.module.css';

interface PaymentSweepActionProps {
  federationId: string;
  /** `null` means the daemon could not read this wallet, which is not the same
   *  as an empty one — an unread balance never blocks the button. */
  balanceMsat: number | null;
  hasDestination: boolean;
}

// Why this sweep cannot be pressed, or null when it can. A balance of `null` is
// an unread wallet rather than an empty one, so it never blocks: the daemon is
// the authority on whether there is anything there.
const readBlockedReason = (hasDestination: boolean, balanceMsat: number | null): string | null => {
  if (!hasDestination) return 'Set a payout destination first.';
  if (balanceMsat === 0) return 'This wallet holds nothing to sweep.';
  return null;
};

/**
 * The whole of setup-payment money-out: one press, no amount, no gateway. The
 * sweep takes the largest economically fundable amount, because an exact amount
 * can fail on mint and routing fees, and the gateway is selected by the daemon
 * (crates/fman/core/src/admin.rs:63).
 */
export const PaymentSweepAction = ({
  federationId,
  balanceMsat,
  hasDestination
}: PaymentSweepActionProps) => {
  const sweep = useSweepPaymentFees(federationId);
  const noteId = useId();
  const blockedReason = readBlockedReason(hasDestination, balanceMsat);

  const handleSweep = () => {
    sweep.mutate();
  };

  return (
    <div className={styles.root}>
      <Button
        size="small"
        disabled={blockedReason !== null}
        loading={sweep.isPending}
        describedBy={blockedReason ? noteId : undefined}
        onClick={handleSweep}
      >
        Sweep
      </Button>

      {blockedReason && (
        <span id={noteId} className={styles.note}>
          {blockedReason}
        </span>
      )}

      {sweep.isSuccess && (
        <span className={styles.outcome}>
          {describePayout(sweep.data)}

          {sweep.data.operation && (
            <span className={styles.operation}>
              <span className={styles.operationId}>
                {truncateMiddle(sweep.data.operation.operation_id, 6, 6)}
              </span>

              <CopyButton value={sweep.data.operation.operation_id} label="Copy operation ID" />
            </span>
          )}
        </span>
      )}

      {sweep.isError && (
        <span role="alert" className={styles.error}>
          {describeActionError(sweep.error)}
        </span>
      )}
    </div>
  );
};
