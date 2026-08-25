import {
  Chip,
  type Column,
  CopyButton,
  DataTable,
  isTruncated,
  truncateMiddle
} from '@operator-ui/common-ui';
import { Link } from 'react-router-dom';
import type { SeatRow } from '@/features/seats/hooks/use-seat-rows/useSeatRows';
import { describePlan } from '@/shared/utils/describePlan';
import { formatDate } from '@/shared/utils/format';
import { describeSeatPhase, describeSeatReport } from '@/shared/utils/seatStatus';
import styles from './SeatTable.module.css';

const columns: Column<SeatRow>[] = [
  {
    key: 'seat',
    header: 'Seat',
    render: (row) => (
      <span className={styles.idRow}>
        <Link to={`/seats/${row.seat.seat_id}`} className={styles.seatLink}>
          {truncateMiddle(row.seat.seat_id, 8, 8)}
        </Link>

        {isTruncated(row.seat.seat_id, 8, 8) && (
          <CopyButton value={row.seat.seat_id} label="Copy seat ID" />
        )}
      </span>
    )
  },
  {
    key: 'fi',
    header: 'FI',
    render: (row) => (
      <span className={styles.idRow}>
        <span className={styles.mono}>{truncateMiddle(row.seat.fi_id, 8, 8)}</span>

        {isTruncated(row.seat.fi_id, 8, 8) && (
          <CopyButton value={row.seat.fi_id} label="Copy FI ID" />
        )}
      </span>
    )
  },
  { key: 'plan', header: 'Plan', render: (row) => describePlan(row.seat.plan) },
  { key: 'created', header: 'Created', render: (row) => formatDate(row.seat.created_at_ms) },
  {
    key: 'phase',
    header: 'Phase',
    render: (row) => {
      if (row.seat.decommissioned) return '—';
      if (!row.report) return row.reportLoading ? '…' : '—';
      return row.report.state === 'active' ? describeSeatPhase(row.report.phase) : '—';
    }
  },
  {
    key: 'health',
    header: 'Health',
    render: (row) => {
      if (row.seat.decommissioned) return '—';
      if (!row.report) return row.reportLoading ? '…' : '—';
      const { label, tone } = describeSeatReport(row.report);
      return <Chip tone={tone}>{label}</Chip>;
    }
  }
];

const seatRowKey = (row: SeatRow) => row.seat.seat_id;

interface SeatTableProps {
  rows: SeatRow[];
}

export const SeatTable = ({ rows }: SeatTableProps) => (
  <div className={styles.tableWrap}>
    <DataTable columns={columns} rows={rows} rowKey={seatRowKey} />
  </div>
);
