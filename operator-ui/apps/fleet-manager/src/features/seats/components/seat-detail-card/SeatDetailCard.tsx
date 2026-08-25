import { CopyButton, isTruncated, truncateMiddle } from '@operator-ui/common-ui';
import type { CompletionCallbackStatus, SeatStatusResponse } from '@operator-ui/types';
import { describePlan } from '@/shared/utils/describePlan';
import { formatDate } from '@/shared/utils/format';
import { describeSeatPhase } from '@/shared/utils/seatStatus';
import styles from './SeatDetailCard.module.css';

interface SeatDetailCardProps {
  seat: SeatStatusResponse;
}

const describeCallback = (callback: CompletionCallbackStatus): string => {
  switch (callback.state) {
    case 'not_configured':
      return 'Not configured';
    case 'pending':
      return `Pending (${callback.attempts} attempts)`;
    case 'operator_blocked':
      return `Operator blocked: ${callback.reason}`;
    case 'delivered':
      return `Delivered (${callback.attempts} attempts)`;
    case 'terminal':
      return `Terminal: ${callback.reason}`;
  }
};

export const SeatDetailCard = ({ seat }: SeatDetailCardProps) => {
  const { report } = seat;

  return (
    <dl className={styles.kv}>
      <div className={styles.kvRow}>
        <dt className={styles.kvLabel}>FI</dt>

        <dd className={styles.idRow}>
          <span className={styles.kvValueMono}>{truncateMiddle(seat.fi_id, 8, 8)}</span>

          {isTruncated(seat.fi_id, 8, 8) && <CopyButton value={seat.fi_id} label="Copy FI ID" />}
        </dd>
      </div>

      <div className={styles.kvRow}>
        <dt className={styles.kvLabel}>Plan</dt>

        <dd className={styles.kvValue}>{describePlan(seat.plan)}</dd>
      </div>

      <div className={styles.kvRow}>
        <dt className={styles.kvLabel}>Created</dt>

        <dd className={styles.kvValue}>{formatDate(seat.created_at_ms)}</dd>
      </div>

      <div className={styles.kvRow}>
        <dt className={styles.kvLabel}>Completion callback</dt>

        <dd className={styles.kvValue}>{describeCallback(seat.completion_callback)}</dd>
      </div>
      {report.state === 'active' && (
        <div className={styles.kvRow}>
          <dt className={styles.kvLabel}>Phase</dt>

          <dd className={styles.kvValue}>{describeSeatPhase(report.phase)}</dd>
        </div>
      )}
      {report.state === 'active' &&
        (report.phase === 'running' || report.phase === 'data_loss') && (
          <div className={styles.kvRow}>
            <dt className={styles.kvLabel}>Invite code</dt>

            <dd className={styles.idRow}>
              <span className={styles.kvValueMono}>{report.invite_code}</span>

              <CopyButton value={report.invite_code} label="Copy invite code" />
            </dd>
          </div>
        )}
      {report.state === 'decommissioned' && (
        <div className={styles.kvRow}>
          <dt className={styles.kvLabel}>Decommissioned</dt>

          <dd className={styles.kvValue}>{formatDate(report.at_ms)}</dd>
        </div>
      )}
    </dl>
  );
};
