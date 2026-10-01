import { SectionCard } from '@operator-ui/common-ui';
import { SeatReadinessCard } from '@/features/seat-readiness/SeatReadinessCard';
import { useSeatReadiness } from '@/shared/api/hooks/use-seat-readiness/useSeatReadiness';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import styles from './HealthPage.module.css';

export const HealthPage = () => {
  const readiness = useSeatReadiness();
  const { disposition, retry } = useQueryDisposition([readiness]);

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
            {readiness.data && <SeatReadinessCard report={readiness.data.report} />}
          </QuerySurface>
        </div>
      </SectionCard>
    </div>
  );
};
