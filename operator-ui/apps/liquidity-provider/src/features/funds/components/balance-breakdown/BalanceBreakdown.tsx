import { SectionCard } from '@operator-ui/common-ui';
import type { BalanceRow } from '@/features/funds/utils/deriveFunds';
import { formatSats } from '@/shared/utils/format';
import styles from './BalanceBreakdown.module.css';

interface BalanceBreakdownProps {
  rows: BalanceRow[];
}

const renderRow = (row: BalanceRow) => (
  <div key={row.key} className={styles.kvRow}>
    <dt className={styles.kvKey}>{row.label}</dt>

    <dd className={styles.kvValue} data-strong={row.strong ? '' : undefined}>
      {formatSats(row.value)}
    </dd>
  </div>
);

export const BalanceBreakdown = ({ rows }: BalanceBreakdownProps) => (
  <SectionCard title="Balance breakdown">
    <dl className={styles.kv}>{rows.map(renderRow)}</dl>

    <p className={styles.note}>Top-ups are manual — FLIP never moves funds in on its own.</p>
  </SectionCard>
);
