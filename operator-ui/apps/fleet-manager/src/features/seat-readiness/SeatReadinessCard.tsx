import { Banner } from '@operator-ui/common-ui';
import type { ShowSeatReadinessResponse } from '@operator-ui/types';
import { formatCheckedAt } from '@/shared/utils/format';
import { CHECKS, guidance, isStale, labels, outcomes, passed } from '@/shared/utils/seatReadiness';
import styles from './SeatReadinessCard.module.css';

interface SeatReadinessCardProps {
  readiness: ShowSeatReadinessResponse;
  nowMs: number;
}

export const SeatReadinessCard = ({ readiness, nowMs }: SeatReadinessCardProps) => {
  const { ready_for_new_seats: ready, report } = readiness;
  return (
    <>
      {ready ? (
        <Banner variant="success" title="Readiness checks passed">
          New seats are offered while a price and free capacity are set.
        </Banner>
      ) : (
        <Banner variant="error" title="Not accepting new seats">
          This host stopped advertising and quoting new seats until every check passes. Running
          seats are not affected.
        </Banner>
      )}

      {report === null ? (
        <p className={styles.checkedAt}>
          No check has finished since this host started. Until one does, the last stored result
          applies.
        </p>
      ) : (
        <>
          {isStale(report, nowMs) && (
            <Banner variant="warn" title="Checks may have stopped">
              The last check finished more than 15 minutes ago. Restart the FMan if this persists.
            </Banner>
          )}

          <ul className={styles.checks}>
            {CHECKS.map((check) => (
              <li key={check} className={styles.check}>
                <strong>{labels[check]}</strong>

                <span>{outcomes[report[check]]}</span>
                {!passed(report[check]) && (
                  <span className={styles.guidance}>{guidance[check]}</span>
                )}
              </li>
            ))}
          </ul>

          <p className={styles.checkedAt}>
            Last checked {formatCheckedAt(Math.floor(report.checked_at_ms / 1000))}. Checks repeat
            every 10 minutes, or every minute while one fails.
          </p>
        </>
      )}
    </>
  );
};
