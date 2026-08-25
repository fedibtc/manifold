import { Button, CopyButton, truncateMiddle } from '@operator-ui/common-ui';
import { useId } from 'react';
import { useCollectGuardianFees } from '@/features/payouts/api/hooks/use-collect-guardian-fees/useCollectGuardianFees';
import { useSweepGuardianFees } from '@/features/payouts/api/hooks/use-sweep-guardian-fees/useSweepGuardianFees';
import { describeCollection, describePayout } from '@/features/payouts/utils/sweepOutcome';
import { describeActionError } from '@/shared/utils/describeActionError';
import styles from './GuardianFeeActions.module.css';

interface GuardianFeeActionsProps {
  seatId: string;
  /** In the pool. `null` when the fee account has not been read. */
  collectableMsat: number | null;
  /** Collected ecash, the only money step two can send. `null` reads as unknown. */
  collectedEcashMsat: number | null;
  hasDestination: boolean;
}

// Collect needs no payout destination: it moves money out of the pool into the
// fleet's own ecash, and nothing leaves the fleet
// (crates/fman/core/src/fleet.rs:1222). Only the send does.
const readCollectBlock = (collectableMsat: number | null): string | null =>
  collectableMsat === 0 ? 'Nothing in the pool to collect.' : null;

const readSendBlock = (
  hasDestination: boolean,
  collectedEcashMsat: number | null
): string | null => {
  if (!hasDestination) return 'Set a payout destination first.';
  if (collectedEcashMsat === 0) return 'Nothing collected yet. Collect first.';
  return null;
};

/**
 * Guardian-fee money-out, which takes two steps and must look like two steps:
 * `CollectGuardianFees` moves what the pool will release into ordinary ecash,
 * then `SweepGuardianFees` sends that ecash to the destination. Presenting them
 * as one control would hide that a collection leaves locked deposits behind
 * until the next cycle turnover.
 */
export const GuardianFeeActions = ({
  seatId,
  collectableMsat,
  collectedEcashMsat,
  hasDestination
}: GuardianFeeActionsProps) => {
  const collect = useCollectGuardianFees(seatId);
  const send = useSweepGuardianFees(seatId);
  const collectNoteId = useId();
  const sendNoteId = useId();
  const collectBlock = readCollectBlock(collectableMsat);
  const sendBlock = readSendBlock(hasDestination, collectedEcashMsat);

  const handleCollect = () => {
    collect.mutate();
  };

  const handleSend = () => {
    send.mutate();
  };

  return (
    <div className={styles.root}>
      <div className={styles.step}>
        <Button
          size="small"
          variant="secondary"
          disabled={collectBlock !== null}
          loading={collect.isPending}
          describedBy={collectBlock ? collectNoteId : undefined}
          onClick={handleCollect}
        >
          1. Collect out of the pool
        </Button>

        {collectBlock && (
          <span id={collectNoteId} className={styles.note}>
            {collectBlock}
          </span>
        )}

        {collect.isSuccess && (
          <span className={styles.outcome}>{describeCollection(collect.data)}</span>
        )}

        {collect.isError && (
          <span role="alert" className={styles.error}>
            {describeActionError(collect.error)}
          </span>
        )}
      </div>

      <div className={styles.step}>
        <Button
          size="small"
          disabled={sendBlock !== null}
          loading={send.isPending}
          describedBy={sendBlock ? sendNoteId : undefined}
          onClick={handleSend}
        >
          2. Send to destination
        </Button>

        {sendBlock && (
          <span id={sendNoteId} className={styles.note}>
            {sendBlock}
          </span>
        )}

        {send.isSuccess && (
          <span className={styles.outcome}>
            {describePayout(send.data)}

            {send.data.operation && (
              <span className={styles.operation}>
                <span className={styles.operationId}>
                  {truncateMiddle(send.data.operation.operation_id, 6, 6)}
                </span>

                <CopyButton value={send.data.operation.operation_id} label="Copy operation ID" />
              </span>
            )}
          </span>
        )}

        {send.isError && (
          <span role="alert" className={styles.error}>
            {describeActionError(send.error)}
          </span>
        )}
      </div>
    </div>
  );
};
