import { EarningsRow } from '@/features/overview/components/earnings-row/EarningsRow';
import type {
  EarningEvent,
  EarningsDay as EarningsDayModel
} from '@/features/overview/utils/deriveEarnings';
import { formatSats } from '@/shared/utils/format';
import styles from './EarningsDay.module.css';

// A remittance whose sealed breakdown would not open carries no payer timestamp,
// so it cannot claim a place in the timeline — but it is still money we were paid.
const UNDATED_LABEL = 'Date unavailable';

const renderEvent = (event: EarningEvent) => <EarningsRow key={event.key} event={event} />;

interface EarningsDayProps {
  bucket: EarningsDayModel;
}

export const EarningsDay = ({ bucket }: EarningsDayProps) => (
  <div className={styles.root}>
    <div className={styles.head}>
      <span className={styles.label}>{bucket.day ?? UNDATED_LABEL}</span>

      <span className={styles.total}>{formatSats(bucket.totalMsat)}</span>
    </div>

    <ul className={styles.events}>{bucket.events.map(renderEvent)}</ul>
  </div>
);
