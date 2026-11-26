import { Banner } from '@operator-ui/common-ui';
import type { ShowSeatReadinessResponse } from '@operator-ui/types';
import { formatCheckedAt } from '@/shared/utils/format';
import {
  describeOutcome,
  formatMillis,
  formatPermille,
  guidance,
  labels,
  orderingLatencyGuidance,
  orderingLatencyLabel,
  PRESSURE_RESOURCES,
  PRESSURE_WINDOWS,
  passed,
  pressureGuidance,
  pressureHigh,
  pressureLabels,
  reportedChecks,
  UPTIME_WINDOWS,
  updateGuidance,
  uptimeLabels
} from '@/shared/utils/seatReadiness';
import { formatWait } from './formatWait';
import styles from './SeatReadinessCard.module.css';

interface SeatReadinessCardProps {
  readiness: ShowSeatReadinessResponse;
}

/** Bars show 50–100%: under 50% a window is far from passing anyway. */
const barPosition = (permille: number) => `${Math.max(0, (permille - 500) / 5)}%`;

/** Pressure and consensus speed bars show 0 to twice the maximum, so the maximum's tick sits in
 * the middle. */
const pressurePosition = (permille: number, maximum: number) =>
  `${Math.min(100, (permille / (2 * maximum)) * 100)}%`;

export const SeatReadinessCard = ({ readiness }: SeatReadinessCardProps) => {
  const { ready_for_new_seats: ready, report } = readiness;
  // Absent from daemons that predate the figures, null when a run could not record them.
  const uptime = report?.uptime_comparison ?? null;
  // Absent from daemons that predate the check, null without figures this run.
  const pressure = report?.host_pressure ?? null;
  const maximums = report?.max_host_pressure ?? null;
  const pressureBars = pressure && maximums ? { pressure, maximums } : null;
  // Absent from daemons that predate the check, null without a figure this run.
  const latency = report?.ordering_latency_6h_ms ?? null;
  const maxLatency = report?.max_ordering_latency_6h_ms ?? null;
  const latencyBar =
    latency !== null && maxLatency !== null
      ? { latency, maximum: maxLatency, ok: latency <= maxLatency }
      : null;
  // With figures, the uptime, pressure and consensus speed bars replace their generic checks.
  const reported = report ? reportedChecks(report) : [];
  const checks = reported.filter(
    (check) =>
      !(check === 'uptime' && uptime) &&
      !(check === 'pressure' && pressureBars) &&
      !(check === 'ordering_latency' && latencyBar)
  );
  const wait = uptime ? Math.max(...UPTIME_WINDOWS.map((window) => uptime.wait_secs[window])) : 0;
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
          {report?.version === 'mg_version_too_old' && ` ${updateGuidance}`}
          {pressureHigh(report?.pressure) && ` ${pressureGuidance}`}
          {report?.ordering_latency === 'ordering_latency_high' && ` ${orderingLatencyGuidance}`}
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

                <span>{describeOutcome(check, report[check])}</span>
                {check !== 'version' &&
                  check !== 'ordering_latency' &&
                  !pressureHigh(report[check]) &&
                  report[check] !== 'host_pressure_measuring' &&
                  !passed(report[check]) && (
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
                    ? `New seats reopen after ${formatWait(wait)} online without a break.`
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

                    {/* <meter> is the semantic element, but it cannot hold the minimum tick
                        and its bar is not styleable across browsers. role="meter" with the
                        aria-value attributes is the same to assistive tech. */}
                    {/* biome-ignore lint/a11y/useSemanticElements: see the note above */}
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
                        Passes after {formatWait(uptime.wait_secs[window])} online without a break.
                      </span>
                    )}
                  </div>
                );
              })}
            </section>
          )}

          {pressureBars && (
            <section className={styles.uptime} aria-label={labels.pressure}>
              <div className={styles.uptimeHead}>
                <strong>{labels.pressure}</strong>

                <span className={styles.guidance}>
                  {passed(report.pressure)
                    ? 'Every average is within its maximum.'
                    : 'New seats reopen once every average is within its maximum.'}
                </span>
              </div>
              {PRESSURE_RESOURCES.flatMap((resource) =>
                PRESSURE_WINDOWS.map((window) => {
                  const average = pressureBars.pressure[resource][window];
                  const maximum = pressureBars.maximums[resource][window];
                  const ok = average <= maximum;
                  const label = pressureLabels[resource][window];
                  return (
                    <div key={`${resource}-${window}`} className={styles.gauge}>
                      <div className={styles.gaugeHead}>
                        <span>{label}</span>

                        <span className={styles.figure}>
                          <span className={ok ? styles.ok : styles.bad}>
                            {formatPermille(average)}
                          </span>

                          <span className={styles.min}> / {formatPermille(maximum)} maximum</span>
                        </span>
                      </div>

                      {/* biome-ignore lint/a11y/useSemanticElements: as for the uptime bars */}
                      <div
                        className={styles.track}
                        role="meter"
                        aria-label={label}
                        aria-valuemin={0}
                        aria-valuemax={100}
                        aria-valuenow={average / 10}
                      >
                        <div
                          className={ok ? styles.fillOk : styles.fillBad}
                          style={{ width: pressurePosition(average, maximum) }}
                        />

                        <div className={styles.tick} style={{ left: '50%' }} />
                      </div>
                    </div>
                  );
                })
              )}
            </section>
          )}

          {latencyBar && (
            <section className={styles.uptime} aria-label={labels.ordering_latency}>
              <div className={styles.uptimeHead}>
                <strong>{labels.ordering_latency}</strong>

                <span className={styles.guidance}>
                  {latencyBar.ok
                    ? 'The average is within its maximum.'
                    : 'New seats reopen once the average is within its maximum.'}
                </span>
              </div>

              <div className={styles.gauge}>
                <div className={styles.gaugeHead}>
                  <span>{orderingLatencyLabel}</span>

                  <span className={styles.figure}>
                    <span className={latencyBar.ok ? styles.ok : styles.bad}>
                      {formatMillis(latencyBar.latency)}
                    </span>

                    <span className={styles.min}>
                      {' '}
                      / {formatMillis(latencyBar.maximum)} maximum
                    </span>
                  </span>
                </div>

                {/* biome-ignore lint/a11y/useSemanticElements: as for the uptime bars */}
                <div
                  className={styles.track}
                  role="meter"
                  aria-label={orderingLatencyLabel}
                  aria-valuemin={0}
                  aria-valuemax={(2 * latencyBar.maximum) / 1000}
                  aria-valuenow={latencyBar.latency / 1000}
                >
                  <div
                    className={latencyBar.ok ? styles.fillOk : styles.fillBad}
                    style={{ width: pressurePosition(latencyBar.latency, latencyBar.maximum) }}
                  />

                  <div className={styles.tick} style={{ left: '50%' }} />
                </div>
              </div>
            </section>
          )}

          <p className={styles.checkedAt}>
            Last checked {formatCheckedAt(Math.floor(report.checked_at_ms / 1000))}. Checks repeat
            every 10 minutes, or every minute while one fails.
            {uptime &&
              ' Uptime is a moving average of these checks, and a new outage lowers it again.'}
            {pressureBars &&
              ' Pressure is the share of time this host’s tasks waited for CPU, disk or memory, as moving averages.'}
            {latencyBar &&
              ' Consensus speed is how long this host’s guardians take to get their own consensus items ordered: the middle guardian’s time, as a moving average.'}
          </p>
        </>
      )}
    </>
  );
};
