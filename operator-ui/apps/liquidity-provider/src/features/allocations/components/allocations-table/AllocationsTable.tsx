import { type Column, CopyButton, DataTable, isTruncated } from '@operator-ui/common-ui';
import type { AdminAllocationSummary } from '@operator-ui/types';
import { AllocationIdButton } from '@/features/allocations/components/allocation-id-button/AllocationIdButton';
import { AllocationStatusChip } from '@/features/allocations/components/allocation-status-chip/AllocationStatusChip';
import { summaryStatus } from '@/shared/utils/allocationStatus';
import { formatAmount } from '@/shared/utils/format';
import styles from './AllocationsTable.module.css';

interface AllocationsTableProps {
  rows: AdminAllocationSummary[];
  selectedId: string | null;
  onSelect: (id: string) => void;
}

const allocationRowKey = (row: AdminAllocationSummary) => row.federation_id;

// Rows only. Whether the list could be loaded at all is the page's question —
// this component is reached with an answer already in hand, so "no rows" here
// always means the daemon reported none.
export const AllocationsTable = ({ rows, selectedId, onSelect }: AllocationsTableProps) => {
  if (rows.length === 0) return <p className={styles.state}>No allocations yet.</p>;

  const columns: Column<AdminAllocationSummary>[] = [
    {
      key: 'federation',
      header: 'Federation',
      render: (row) => (
        <span className={styles.idRow}>
          <AllocationIdButton
            id={row.federation_id}
            selected={row.federation_id === selectedId}
            onSelect={onSelect}
          />

          {isTruncated(row.federation_id, 8, 8) && (
            <CopyButton value={row.federation_id} label="Copy federation ID" />
          )}
        </span>
      )
    },
    {
      key: 'committed',
      header: 'Committed (SATS)',
      render: (row) => <span className={styles.amount}>{formatAmount(row.committed_amount)}</span>
    },
    {
      key: 'status',
      header: 'Status',
      render: (row) => <AllocationStatusChip status={summaryStatus(row)} />
    }
  ];

  return (
    <div className={styles.allocationsTable}>
      <DataTable columns={columns} rows={rows} rowKey={allocationRowKey} />
    </div>
  );
};
