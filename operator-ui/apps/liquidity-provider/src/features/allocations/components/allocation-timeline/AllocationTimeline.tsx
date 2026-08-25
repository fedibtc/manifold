import { Banner, Button } from '@operator-ui/common-ui';
import type {
  AdminAllocationDetail,
  ItemAllocationStatus,
  WalletOperation,
  WalletOperationStatus,
  WalletOperationType
} from '@operator-ui/types';
import { type ReactNode, useState } from 'react';
import { useCancelAllocation } from '@/features/allocations/api/hooks/use-cancel-allocation/useCancelAllocation';
import { useRetryFundingStep } from '@/features/allocations/api/hooks/use-retry-funding-step/useRetryFundingStep';
import { detailStatus } from '@/shared/utils/allocationStatus';
import { describeActionError } from '@/shared/utils/describeActionError';
import { formatAmount } from '@/shared/utils/format';
import styles from './AllocationTimeline.module.css';

interface AllocationTimelineProps {
  detail: AdminAllocationDetail;
}

const OPERATION_LABELS: Record<WalletOperationType, string> = {
  deposit: 'Deposit',
  withdrawal: 'Withdrawal',
  gateway_funding: 'Gateway funding',
  stability_pool_funding: 'Stability pool funding'
};

const STATUS_LABELS: Record<WalletOperationStatus, string> = {
  pending: 'Pending',
  broadcast: 'Broadcast',
  confirmed: 'Confirmed',
  completed: 'Completed',
  in_doubt: 'In doubt',
  manual_review_required: 'Manual review required',
  failed: 'Failed',
  cancelled: 'Cancelled'
};

const CANCELLABLE_STATUSES: ItemAllocationStatus[] = [
  'pending',
  'running',
  'action_required',
  'failed'
];

export const AllocationTimeline = ({ detail }: AllocationTimelineProps) => {
  const { federation_id, wallet_operations, failures } = detail;
  const hasSteps = wallet_operations.length > 0;
  const canCancel = CANCELLABLE_STATUSES.includes(detailStatus(detail));

  const [confirmingCancel, setConfirmingCancel] = useState(false);
  const retryFundingStep = useRetryFundingStep();
  const cancelAllocation = useCancelAllocation();

  const handleRetry = (operation: WalletOperation) => {
    retryFundingStep.mutate({
      federation_id,
      item_id: operation.item_id ?? null,
      operation_id: operation.operation_id
    });
  };

  const handleCancelStart = () => setConfirmingCancel(true);
  const handleCancelBack = () => setConfirmingCancel(false);
  const handleCancelConfirm = () => {
    setConfirmingCancel(false);
    cancelAllocation.mutate({ federation_id, reason: null });
  };

  let retryBanner: ReactNode = null;
  if (retryFundingStep.isError) {
    retryBanner = (
      <Banner variant="error" title="Retry failed">
        {describeActionError(retryFundingStep.error)}
      </Banner>
    );
  } else if (retryFundingStep.data) {
    retryBanner =
      retryFundingStep.data.status === 'accepted' ? (
        <Banner variant="success">Retry submitted — the daemon will reattempt this step.</Banner>
      ) : (
        <Banner variant="error" title="Retry not applied">
          {retryFundingStep.data.detail ?? 'The daemon could not retry this step.'}
        </Banner>
      );
  }

  let cancelBanner: ReactNode = null;
  if (cancelAllocation.isError) {
    cancelBanner = (
      <Banner variant="error" title="Cancel failed">
        {describeActionError(cancelAllocation.error)}
      </Banner>
    );
  } else if (cancelAllocation.data) {
    cancelBanner =
      cancelAllocation.data.status === 'accepted' ? (
        <Banner variant="success">Allocation cancelled.</Banner>
      ) : (
        <Banner variant="error" title="Cancel not applied">
          {cancelAllocation.data.detail ?? 'The daemon could not cancel this allocation.'}
        </Banner>
      );
  }

  return (
    <div className={styles.root}>
      {hasSteps ? (
        <ol className={styles.steps}>
          {wallet_operations.map((operation) => (
            <li key={operation.operation_id} className={styles.step}>
              <span className={styles.marker} data-status={operation.status} />

              <div className={styles.body}>
                <span className={styles.title}>{OPERATION_LABELS[operation.operation_type]}</span>

                <span className={styles.status} data-status={operation.status}>
                  {STATUS_LABELS[operation.status]}
                </span>
              </div>

              <span className={styles.amount}>{formatAmount(operation.amount)} SATS</span>

              {operation.status === 'failed' && (
                <span className={styles.retryAction}>
                  <Button
                    variant="secondary"
                    size="small"
                    loading={retryFundingStep.isPending}
                    onClick={() => handleRetry(operation)}
                  >
                    Retry
                  </Button>
                </span>
              )}
            </li>
          ))}
        </ol>
      ) : (
        <p className={styles.empty}>No wallet operations recorded.</p>
      )}

      {retryBanner}

      {failures.length > 0 ? (
        <ul className={styles.failures}>
          {failures.map((failure) => (
            <li key={`${failure.code}-${failure.occurred_at}`} className={styles.failure}>
              <span className={styles.failureCode}>{failure.code}</span>

              <span className={styles.failureMessage}>{failure.message}</span>
            </li>
          ))}
        </ul>
      ) : null}

      <p className={styles.note}>
        Retry re-attempts a failed step. Cancel stops a pending, running, or failed allocation
        &mdash; it can&apos;t be undone.
      </p>

      {canCancel && (
        <div className={styles.cancelSection}>
          {confirmingCancel ? (
            <div className={styles.cancelConfirm}>
              <span className={styles.cancelConfirmLabel}>Cancel this allocation?</span>

              <div className={styles.cancelConfirmActions}>
                <Button variant="danger" size="small" onClick={handleCancelConfirm}>
                  Confirm cancel
                </Button>

                <Button variant="secondary" size="small" onClick={handleCancelBack}>
                  Back
                </Button>
              </div>
            </div>
          ) : (
            <Button
              variant="danger"
              size="small"
              loading={cancelAllocation.isPending}
              onClick={handleCancelStart}
            >
              Cancel allocation
            </Button>
          )}

          {cancelBanner}
        </div>
      )}
    </div>
  );
};
