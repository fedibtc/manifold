import { Banner } from '@operator-ui/common-ui';
import type { ReadinessReport } from '@operator-ui/types';
import { formatCheckedAt } from '@/shared/utils/format';
import {
  CHECKS,
  failedChecks,
  guidance,
  labels,
  outcomes,
  passed
} from '@/shared/utils/seatReadiness';
import styles from './SeatReadinessCard.module.css';

interface SeatReadinessCardProps {
  /** Null until the daemon's first readiness run completes. */
  report: ReadinessReport | null;
}

export const SeatReadinessCard = ({ report }: SeatReadinessCardProps) => {
  if (report === null) {
    return (
      <Banner variant="info" title="Checking readiness">
        The first readiness check has not finished. New seats are not offered until it passes.
      </Banner>
    );
  }
  const ready = failedChecks(report).length === 0;
  return (
    <>
      {ready ? (
        <Banner variant="success" title="Accepting new seats">
          Every readiness check passed.
        </Banner>
      ) : (
        <Banner variant="error" title="Not accepting new seats">
          This host stopped advertising and quoting new seats until every check passes. Running
          seats are not affected.
        </Banner>
      )}

      <ul className={styles.checks}>
        {CHECKS.map((check) => (
          <li key={check} className={styles.check} data-passed={passed(report[check])}>
            <strong>{labels[check]}</strong>

            <span>{outcomes[report[check]]}</span>
            {!passed(report[check]) && <span className={styles.guidance}>{guidance[check]}</span>}
          </li>
        ))}
      </ul>

      <p className={styles.checkedAt}>
        Last checked {formatCheckedAt(Math.floor(report.checked_at_ms / 1000))}. Checks repeat every
        10 minutes, or every minute while one fails.
      </p>
    </>
  );
};
