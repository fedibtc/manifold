import { Chip, type Column, DataTable, SectionCard } from '@operator-ui/common-ui';
import type { PayoutJobStatusResponse } from '@operator-ui/types';
import { describeSource, sourceOfScope } from '@/features/payouts/utils/payoutSource';
import { payoutMethodLabel, payoutStage } from '@/features/payouts/utils/payoutStage';
import { formatLocalMoment, formatSats } from '@/shared/utils/format';
import styles from './PayoutHistoryTable.module.css';

interface PayoutHistoryTableProps {
  /** Final payouts, newest first. */
  payouts: PayoutJobStatusResponse[];
  /** Open one payout's dialog at its receipt, with its operation ID. */
  onOpen: (requestId: string) => void;
}

const rowKey = (payout: PayoutJobStatusResponse) => payout.job.request_id;

/** Every finished withdrawal of both methods; each opens its receipt. */
export const PayoutHistoryTable = ({ payouts, onOpen }: PayoutHistoryTableProps) => {
  const columns: Column<PayoutJobStatusResponse>[] = [
    {
      key: 'when',
      header: 'When',
      render: (payout) => (
        <button
          type="button"
          className={styles.open}
          onClick={() => onOpen(payout.job.request_id)}
          aria-label={`Open the withdrawal of ${formatSats(payout.job.operation?.amount_msat)} from ${formatLocalMoment(payout.job.created_at_ms / 1000)}`}
        >
          {formatLocalMoment(payout.job.created_at_ms / 1000)}
        </button>
      )
    },
    {
      key: 'amount',
      header: 'Amount',
      render: (payout) => (
        <span className={styles.nowrap}>{formatSats(payout.job.operation?.amount_msat)}</span>
      )
    },
    {
      key: 'from',
      header: 'From',
      render: (payout) => (
        <span className={styles.nowrap}>{describeSource(sourceOfScope(payout.job.scope))}</span>
      )
    },
    { key: 'how', header: 'How', render: payoutMethodLabel },
    {
      key: 'status',
      header: 'Status',
      render: (payout) => {
        const stage = payoutStage(payout);
        return <Chip tone={stage.tone}>{stage.label}</Chip>;
      }
    }
  ];

  return (
    <SectionCard title="History" frame="table">
      {payouts.length === 0 ? (
        <p className={styles.empty}>
          No withdrawals yet. Each one is listed here with its operation ID.
        </p>
      ) : (
        <DataTable columns={columns} rows={payouts} rowKey={rowKey} />
      )}
    </SectionCard>
  );
};
