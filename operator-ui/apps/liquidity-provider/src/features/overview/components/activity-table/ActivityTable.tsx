import { Chip, type ChipTone, type Column, DataTable } from '@operator-ui/common-ui';
import type { ActivityRow } from '@/features/overview/utils/derive';
import styles from './ActivityTable.module.css';

interface ActivityTableProps {
  rows: ActivityRow[];
}

const activityStatusTone = (status: string): ChipTone => {
  const normalised = status.toLowerCase();
  if (normalised.includes('fail') || normalised.includes('doubt')) return 'bad';
  if (normalised.includes('pending') || normalised.includes('broadcast')) return 'warn';
  if (normalised.includes('run') || normalised.includes('fund')) return 'info';
  return 'ok';
};

const activityColumns: Column<ActivityRow>[] = [
  { key: 'when', header: 'When', render: (row) => row.when },
  {
    key: 'event',
    header: 'Event',
    render: (row) => <span className={styles.event}>{row.event}</span>
  },
  { key: 'amount', header: 'Amount', render: (row) => row.amount },
  {
    key: 'status',
    header: 'Status',
    render: (row) => <Chip tone={activityStatusTone(row.status)}>{row.status}</Chip>
  }
];

const activityRowKey = (row: ActivityRow) => row.key;

export const ActivityTable = ({ rows }: ActivityTableProps) => (
  <section className={styles.root}>
    <h2 className={styles.title}>Recent activity</h2>
    {rows.length > 0 ? (
      <div className={styles.table}>
        <DataTable columns={activityColumns} rows={rows} rowKey={activityRowKey} />
      </div>
    ) : (
      <p className={styles.empty}>No recent activity yet.</p>
    )}
  </section>
);
