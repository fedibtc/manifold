import {
  Chip,
  type Column,
  CopyButton,
  DataTable,
  isTruncated,
  SectionCard,
  truncateMiddle
} from '@operator-ui/common-ui';
import { GuardianFeeZeroNote } from '@/features/payouts/components/guardian-fee-zero-note/GuardianFeeZeroNote';
import { WithdrawCell } from '@/features/payouts/components/withdraw-cell/WithdrawCell';
import type { GuardianFeeRow } from '@/features/payouts/hooks/use-guardian-fee-rows/useGuardianFeeRows';
import { formatSats } from '@/shared/utils/format';
import styles from './GuardianFeeTable.module.css';

interface GuardianFeeTableProps {
  rows: GuardianFeeRow[];
  onWithdraw: (seatId: string) => void;
}

const seatRowKey = (row: GuardianFeeRow) => row.seatId;

const isEmptyWhileEarning = (row: GuardianFeeRow) =>
  row.collectableMsat === 0 && row.collectedEcashMsat === 0 && row.earning === true;

/**
 * Guardian-fee revenue, which is per seat. The two amount columns are the two
 * places the money can sit: still in the pool, and collected into ecash. A
 * withdrawal takes the second; its dialog collects the first into it.
 */
export const GuardianFeeTable = ({ rows, onWithdraw }: GuardianFeeTableProps) => {
  const everySeatEmptyWhileEarning = rows.length > 0 && rows.every(isEmptyWhileEarning);
  const columns: Column<GuardianFeeRow>[] = [
    {
      key: 'seat',
      header: 'Seat',
      render: (row) => (
        <span className={styles.idRow}>
          <span className={styles.mono}>{truncateMiddle(row.seatId, 8, 8)}</span>

          {isTruncated(row.seatId, 8, 8) && <CopyButton value={row.seatId} label="Copy seat ID" />}

          {row.earning === false && <Chip tone="warn">Fees stopped</Chip>}
        </span>
      )
    },
    {
      key: 'pool',
      header: 'In the fee pool',
      render: (row) => formatSats(row.collectableMsat)
    },
    {
      key: 'ecash',
      header: 'Ready to withdraw',
      render: (row) => formatSats(row.collectedEcashMsat)
    },
    {
      key: 'withdraw',
      header: '',
      render: (row) => (
        <WithdrawCell
          isEmpty={row.collectableMsat === 0 && row.collectedEcashMsat === 0}
          balanceName={`guardian fees of seat ${truncateMiddle(row.seatId, 8, 8)}`}
          onWithdraw={() => onWithdraw(row.seatId)}
        />
      )
    }
  ];

  return (
    <SectionCard title="Guardian fees" frame="table">
      {rows.length === 0 ? (
        <p className={styles.empty}>
          No guardian fees yet. They appear once a federation you guard is running and its members
          start sending payments.
        </p>
      ) : (
        <DataTable columns={columns} rows={rows} rowKey={seatRowKey} />
      )}

      {everySeatEmptyWhileEarning && <GuardianFeeZeroNote />}
    </SectionCard>
  );
};
