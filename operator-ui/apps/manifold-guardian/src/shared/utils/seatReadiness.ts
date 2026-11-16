import type { ReadinessOutcome, ReadinessReport, UptimePermille } from '@operator-ui/types';

/** The checks one daemon readiness run makes, in display order. */
export type ReadinessCheck =
  | 'relay'
  | 'discovery'
  | 'bitcoin'
  | 'version'
  | 'pressure'
  | 'uptime';

export const CHECKS: readonly ReadinessCheck[] = [
  'relay',
  'discovery',
  'bitcoin',
  'version',
  'pressure',
  'uptime'
];

export const labels: Record<ReadinessCheck, string> = {
  relay: 'Relay connection',
  discovery: 'Guardian discovery',
  bitcoin: 'Bitcoin backend',
  version: 'Software version',
  pressure: 'CPU and disk pressure',
  uptime: 'Uptime'
};

export const outcomes: Record<ReadinessOutcome, string> = {
  pass: 'Passed.',
  not_applicable: 'Not checked in this deployment.',
  relay_disconnected: 'This host is not connected to a relay.',
  discovery_record_missing:
    'This host’s discovery record was not found, or it names no relay this host is connected to.',
  bitcoin_unavailable: 'The configured Bitcoin backend did not answer.',
  bitcoin_wrong_network: 'The configured Bitcoin backend is on a different network.',
  bitcoin_syncing: 'Bitcoin Core is still in initial block download.',
  bitcoin_no_fee_rate: 'The Bitcoin backend has no fee estimate yet.',
  mg_version_too_old: 'Update required.',
  host_pressure_high: 'Above the limit for new seats.',
  host_pressure_unavailable: 'This host’s CPU and disk pressure could not be read.',
  uptime_1d_low: 'This host was offline too much recently.',
  uptime_7d_low: 'This host was offline too much recently.',
  uptime_30d_low: 'This host was offline too much recently.'
};

/** What a check that does not apply means for that check. */
const notApplicable: Partial<Record<ReadinessCheck, string>> = {
  version: 'No minimum set.',
  pressure: 'No limit set.'
};

/** The host-facing words for one check's outcome. */
export const describeOutcome = (check: ReadinessCheck, outcome: ReadinessOutcome): string =>
  (outcome === 'not_applicable' && notApplicable[check]) || outcomes[outcome];

/** What to do about a failing check, shown under it. The version check, and a pressure check that
 * holds new seats, have a one-line outcome like the others; what to do about them is in the
 * not-ready banner ({@link updateGuidance}, {@link pressureGuidance}). */
export const guidance: Record<Exclude<ReadinessCheck, 'version'>, string> = {
  relay:
    'Check that this host can hold long-lived outbound connections to the relay. No inbound port is needed.',
  discovery:
    'Check outbound DNS and HTTPS from this host. A new record can take a minute to publish after a relay change.',
  bitcoin:
    'Check the configured Bitcoin backend, its credentials, and its network. Let initial sync finish; do not expose RPC publicly.',
  pressure:
    'Manifold Fedimint Guardian reads pressure from its cgroup, which needs Linux 4.20 or later with cgroup v2. Contact support if this persists.',
  uptime: 'Keep this host online.'
};

const percentOf = (permille: number | null) =>
  permille === null ? 'not enough history yet' : `${(permille / 10).toFixed(1)}%`;

/** The pressure figures the pressure check compared, or nothing where the daemon could not read
 * them or predates the check. A held host reopens on the 24-hour averages, so those are the figures
 * it is waiting on. */
export const describeHostPressure = (report: ReadinessReport): string | null => {
  const pressure = report.host_pressure;
  if (pressure === null || pressure === undefined) return null;
  const limit = report.host_pressure_limit;
  if (report.pressure === 'host_pressure_high' && limit) {
    return `24-hour average: CPU ${percentOf(pressure.cpu['24h'])}, disk ${percentOf(pressure.io['24h'])}. New seats reopen at CPU ${limit.cpu.resume_below}% and disk ${limit.io.resume_below}% or less.`;
  }
  const now = `6-hour average: CPU ${percentOf(pressure.cpu['6h'])}, disk ${percentOf(pressure.io['6h'])}.`;
  return limit
    ? `${now} New seats stop above CPU ${limit.cpu.block_above}% or disk ${limit.io.block_above}%.`
    : now;
};

export const updateGuidance =
  'This version of Manifold Guardian is below the minimum for new seats. Update to the latest release; this does not count against uptime.';

export const pressureGuidance =
  'This host has been too busy for more seats: its tasks spent too long waiting for CPU or disk. Move other work off it, or add CPU or faster disks; this does not count against uptime.';

export const passed = (outcome: ReadinessOutcome): boolean =>
  outcome === 'pass' || outcome === 'not_applicable';

/** The checks a report carries: a daemon that predates a check has no outcome for it. */
export const reportedChecks = (report: ReadinessReport): ReadinessCheck[] =>
  CHECKS.filter((check) => report[check] !== undefined);

export const failedChecks = (report: ReadinessReport): ReadinessCheck[] =>
  reportedChecks(report).filter((check) => !passed(report[check]));

/** The uptime windows the gate compares, shortest first. */
export const UPTIME_WINDOWS: readonly (keyof UptimePermille)[] = ['1d', '7d', '30d'];

export const uptimeLabels: Record<keyof UptimePermille, string> = {
  '1d': 'Last day',
  '7d': 'Last 7 days',
  '30d': 'Last 30 days'
};

/** A per-mille figure as the percent the gate compares, e.g. 899 → "89.9%". */
export const formatPermille = (permille: number): string => `${(permille / 10).toFixed(1)}%`;
