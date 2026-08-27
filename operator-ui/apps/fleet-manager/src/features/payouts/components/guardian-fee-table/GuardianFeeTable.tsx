import {
  type Column,
  CopyButton,
  DataTable,
  isTruncated,
  SectionCard,
  truncateMiddle
} from '@operator-ui/common-ui';
import { GuardianFeeActions } from '@/features/payouts/components/guardian-fee-actions/GuardianFeeActions';
import type { GuardianFeeRow } from '@/features/payouts/hooks/use-guardian-fee-rows/useGuardianFeeRows';
import { formatSats } from '@/shared/utils/format';
import styles from './GuardianFeeTable.module.css';

interface GuardianFeeTableProps {
  rows: GuardianFeeRow[];
  hasDestination: boolean;
}

const seatRowKey = (row: GuardianFeeRow) => row.seatId;

/**
 * Guardian-fee revenue, which is per seat and leaves in two steps. The two
 * amount columns are the two places the money can sit: still in the pool, and
 * collected into ecash. A sweep can only send the second.
 */
export const GuardianFeeTable = ({ rows, hasDestination }: GuardianFeeTableProps) => {
  const columns: Column<GuardianFeeRow>[] = [
    {
      key: 'seat',
      header: 'Seat',
      render: (row) => (
        <span className={styles.idRow}>
          <span className={styles.mono}>{truncateMiddle(row.seatId, 8, 8)}</span>

          {isTruncated(row.seatId, 8, 8) && <CopyButton value={row.seatId} label="Copy seat ID" />}
        </span>
      )
    },
    {
      key: 'pool',
      header: 'In the pool',
      render: (row) => formatSats(row.collectableMsat)
    },
    {
      key: 'ecash',
      header: 'Collected, ready to send',
      render: (row) => formatSats(row.collectedEcashMsat)
    },
    {
      key: 'actions',
      header: 'Move out',
      render: (row) => (
        <GuardianFeeActions
          seatId={row.seatId}
          collectableMsat={row.collectableMsat}
          collectedEcashMsat={row.collectedEcashMsat}
          hasDestination={hasDestination}
        />
      )
    }
  ];

  return (
    <SectionCard title="Guardian-fee revenue" frame="table">
      <DataTable columns={columns} rows={rows} rowKey={seatRowKey} />
    </SectionCard>
  );
};
