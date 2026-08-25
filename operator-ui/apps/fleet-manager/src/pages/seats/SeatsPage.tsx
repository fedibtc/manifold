import { Link } from 'react-router-dom';
import { SeatTable } from '@/features/seats/components/seat-table/SeatTable';
import { useSeatRows } from '@/features/seats/hooks/use-seat-rows/useSeatRows';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import styles from './SeatsPage.module.css';

export const SeatsPage = () => {
  const { rows, activeCount, decommissionedCount, isEmpty, disposition, retry } = useSeatRows();

  const fleet = isEmpty ? (
    <p className={styles.empty}>
      No seats yet. Seats are created by Federation Initiators after they pay for a plan — there is
      no "create seat" action on this dashboard. See <Link to="/offer">Your offer</Link> for the
      current price.
    </p>
  ) : (
    <>
      <p className={styles.summary}>
        {activeCount} active · {decommissionedCount} decommissioned
      </p>

      <SeatTable rows={rows} />
    </>
  );

  return (
    <div className={styles.root}>
      <h1 className={styles.heading}>Seats</h1>

      <QuerySurface disposition={disposition} onRetry={retry}>
        {fleet}
      </QuerySurface>
    </div>
  );
};
