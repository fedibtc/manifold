import { Banner } from '@operator-ui/common-ui';
import type { ShowSeatReadinessResponse } from '@operator-ui/types';
import { formatCheckedAt } from '@/shared/utils/format';
import {
  CHECKS,
  formatPermille,
  guidance,
  labels,
  outcomes,
  passed,
  UPTIME_WINDOWS,
  uptimeLabels
} from '@/shared/utils/seatReadiness';
import styles from './SeatReadinessCard.module.css';
import { daysToRecover, formatDuration } from './uptimeRecovery';

interface SeatReadinessCardProps {
  readiness: ShowSeatReadinessResponse;
}

/** Bars show 50–100%: under 50% a window is far from passing anyway. */
const barPosition = (permille: number) => `${Math.max(0, (permille - 500) / 5)}%`;

export const SeatReadinessCard = ({ readiness }: SeatReadinessCardProps) => {
  const { ready_for_new_seats: ready, report } = readiness;
  // Absent from daemons that predate the figures, null when a run could not record them.
  const uptime = report?.uptime_permille ?? null;
  // With figures, the uptime bars replace the generic uptime check.
  const checks = uptime ? CHECKS.filter((check) => check !== 'uptime') : CHECKS;
  const wait = uptime
    ? Math.max(
        0,
        ...UPTIME_WINDOWS.map((window) =>
          daysToRecover(window, uptime.scores[window], uptime.minimums[window])
        )
      )
    : 0;
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
          <ul className={styles.checks}>
            {checks.map((check) => (
              <li key={check} className={styles.check}>
                <strong>{labels[check]}</strong>

                <span>{outcomes[report[check]]}</span>
                {!passed(report[check]) && (
                  <span className={styles.guidance}>{guidance[check]}</span>
                )}
              </li>
            ))}
          </ul>

          {uptime && (
            <section className={styles.uptime} aria-label="Uptime">
              <div className={styles.uptimeHead}>
                <strong>Uptime</strong>
                <span className={styles.guidance}>
                  {wait > 0
                    ? `New seats reopen after ${formatDuration(wait)} online without a break.`
                    : 'Every window is above its minimum.'}
                </span>
              </div>
              {UPTIME_WINDOWS.map((window) => {
                const score = uptime.scores[window];
                const minimum = uptime.minimums[window];
                const ok = score >= minimum;
                return (
                  <div key={window} className={styles.gauge}>
                    <div className={styles.gaugeHead}>
                      <span>{uptimeLabels[window]}</span>
                      <span className={styles.figure}>
                        <span className={ok ? styles.ok : styles.bad}>{formatPermille(score)}</span>
                        <span className={styles.min}> / {formatPermille(minimum)} needed</span>
                      </span>
                    </div>
                    <div
                      className={styles.track}
                      role="meter"
                      aria-label={uptimeLabels[window]}
                      aria-valuemin={0}
                      aria-valuemax={100}
                      aria-valuenow={score / 10}
                    >
                      <div
                        className={ok ? styles.fillOk : styles.fillBad}
                        style={{ width: barPosition(score) }}
                      />
                      <div className={styles.tick} style={{ left: barPosition(minimum) }} />
                    </div>
                    {!ok && (
                      <span className={styles.guidance}>
                        Passes after{' '}
                        {formatDuration(daysToRecover(window, score, minimum))} online without a
                        break.
                      </span>
                    )}
                  </div>
                );
              })}
            </section>
          )}

          <p className={styles.checkedAt}>
            Last checked {formatCheckedAt(Math.floor(report.checked_at_ms / 1000))}. Checks repeat
            every 10 minutes, or every minute while one fails.
            {uptime &&
              ' Uptime is a moving average of these checks, and a new outage lowers it again.'}
          </p>
        </>
      )}
    </>
  );
};
