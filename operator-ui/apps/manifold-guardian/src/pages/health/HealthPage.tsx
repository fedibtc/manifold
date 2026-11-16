import { SectionCard } from '@operator-ui/common-ui';
import { DataUseSummary } from '@/features/offer/components/data-use-summary/DataUseSummary';
import { SeatReadinessCard } from '@/features/seat-readiness/SeatReadinessCard';
import { useCapacity } from '@/shared/api/hooks/use-capacity/useCapacity';
import { useSeatReadiness } from '@/shared/api/hooks/use-seat-readiness/useSeatReadiness';
import { useSeats } from '@/shared/api/hooks/use-seats/useSeats';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import styles from './HealthPage.module.css';

export const HealthPage = () => {
  const readiness = useSeatReadiness();
  const capacity = useCapacity();
  const seats = useSeats();
  const { disposition, retry } = useQueryDisposition([readiness]);
  const data = useQueryDisposition([capacity, seats]);
  const activeSeats = seats.data
    ? seats.data.seats.filter((seat) => !seat.decommissioned).length
    : 0;

  return (
    <div className={styles.root}>
      <h1 className={styles.heading}>Health</h1>

      <SectionCard title="New seat readiness">
        <div className={styles.readiness}>
          <p>
            This host checks that it could serve a new guardian seat before it advertises or sells
            one.
          </p>

          <QuerySurface disposition={disposition} onRetry={retry}>
            {readiness.data && <SeatReadinessCard readiness={readiness.data} />}
          </QuerySurface>
        </div>
      </SectionCard>

      <SectionCard title="Monthly data">
        <QuerySurface disposition={data.disposition} onRetry={data.retry}>
          {capacity.data && <DataUseSummary capacity={capacity.data} activeSeats={activeSeats} />}
        </QuerySurface>
      </SectionCard>
    </div>
  );
};
