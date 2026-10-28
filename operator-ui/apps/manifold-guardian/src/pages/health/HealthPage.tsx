import { SectionCard } from '@operator-ui/common-ui';
import { ReachCard } from '@/features/reach/components/reach-card/ReachCard';
import { SeatReadinessCard } from '@/features/seat-readiness/SeatReadinessCard';
import { useReach } from '@/shared/api/hooks/use-reach/useReach';
import { useSeatReadiness } from '@/shared/api/hooks/use-seat-readiness/useSeatReadiness';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import styles from './HealthPage.module.css';

export const HealthPage = () => {
  const readiness = useSeatReadiness();
  const readinessState = useQueryDisposition([readiness]);
  const reach = useReach();
  const reachState = useQueryDisposition([reach]);

  return (
    <div className={styles.root}>
      <h1 className={styles.heading}>Health</h1>

      <SectionCard title="New seat readiness">
        <div className={styles.readiness}>
          <p>
            This host checks that it could serve a new guardian seat before it advertises or sells
            one.
          </p>

          <QuerySurface disposition={readinessState.disposition} onRetry={readinessState.retry}>
            {readiness.data && <SeatReadinessCard readiness={readiness.data} />}
          </QuerySurface>
        </div>
      </SectionCard>

      <SectionCard title="Reach">
        <div className={styles.readiness}>
          <p>
            How long each seat's guardian-fee calls take, and whether its guardian reaches the other
            guardians of its federation. A slow call is why a fee screen can say it cannot reach
            this guardian while the guardian is running.
          </p>

          <QuerySurface disposition={reachState.disposition} onRetry={reachState.retry}>
            {reach.data && <ReachCard reach={reach.data} />}
          </QuerySurface>
        </div>
      </SectionCard>
    </div>
  );
};
