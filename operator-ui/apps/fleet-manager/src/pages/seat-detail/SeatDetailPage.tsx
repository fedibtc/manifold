import { Chip } from '@operator-ui/common-ui';
import { Link, useParams } from 'react-router-dom';
import { useSeatStatus } from '@/features/seats/api/hooks/use-seat-status/useSeatStatus';
import { SeatDetailCard } from '@/features/seats/components/seat-detail-card/SeatDetailCard';
import { SeatRecoveryNotices } from '@/features/seats/components/seat-recovery-notices/SeatRecoveryNotices';
import { QuerySurface } from '@/shared/components/query-surface/QuerySurface';
import { useQueryDisposition } from '@/shared/query/use-query-disposition/useQueryDisposition';
import { describeSeatReport } from '@/shared/utils/seatStatus';
import styles from './SeatDetailPage.module.css';

export const SeatDetailPage = () => {
  const { seatId = '' } = useParams<{ seatId: string }>();
  const seat = useSeatStatus(seatId);

  // Every line below the heading is a claim about this seat, so all of it goes
  // through the surface. W3.4 gave the failure branch a retry of its own; the
  // branch that was still missing is "we hold a report and the last poll
  // failed", which used to delete the report the page was already showing.
  const { disposition, retry } = useQueryDisposition([seat]);
  const status = seat.data ? describeSeatReport(seat.data.report) : null;

  const detail =
    seat.data && status ? (
      <>
        <div className={styles.pageHead}>
          <h1 className={styles.heading}>{seat.data.seat_id}</h1>

          <Chip tone={status.tone}>{status.label}</Chip>
        </div>

        <SeatRecoveryNotices report={seat.data.report} />

        <SeatDetailCard seat={seat.data} />
      </>
    ) : null;

  return (
    <div className={styles.root}>
      <QuerySurface disposition={disposition} onRetry={retry}>
        {detail}
      </QuerySurface>

      <Link to="/seats" className={styles.backLink}>
        Back to seats
      </Link>
    </div>
  );
};
