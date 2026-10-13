import { SectionCard } from '@operator-ui/common-ui';
import { EarningsDay } from '@/features/overview/components/earnings-day/EarningsDay';
import type { EarningsDay as EarningsDayModel } from '@/features/overview/utils/deriveEarnings';
import styles from './EarningsTimeline.module.css';

const renderDay = (bucket: EarningsDayModel) => (
  <EarningsDay key={bucket.day ?? 'undated'} bucket={bucket} />
);

interface EarningsTimelineProps {
  days: EarningsDayModel[];
}

export const EarningsTimeline = ({ days }: EarningsTimelineProps) => {
  if (days.length === 0) {
    return (
      <SectionCard title="Earnings">
        <p className={styles.empty}>Nothing earned yet. Seat sales and guardian fees land here.</p>
      </SectionCard>
    );
  }

  return (
    <SectionCard title="Earnings">
      <div className={styles.root}>{days.map(renderDay)}</div>
    </SectionCard>
  );
};
