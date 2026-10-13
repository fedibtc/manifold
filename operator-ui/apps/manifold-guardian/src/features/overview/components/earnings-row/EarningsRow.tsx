import type { EarningEvent } from '@/features/overview/utils/deriveEarnings';
import { formatSats } from '@/shared/utils/format';
import styles from './EarningsRow.module.css';

const kindLabels: Record<EarningEvent['kind'], string> = {
  'seat-sale': 'Seat sold',
  'guardian-fee': 'Guardian fee'
};

interface EarningsRowProps {
  event: EarningEvent;
}

export const EarningsRow = ({ event }: EarningsRowProps) => (
  <li className={styles.root}>
    <div className={styles.body}>
      <span className={styles.title}>{kindLabels[event.kind]}</span>

      <span className={styles.detail}>{event.detail}</span>
    </div>

    <span className={styles.amount}>{formatSats(event.amountMsat)}</span>
  </li>
);
