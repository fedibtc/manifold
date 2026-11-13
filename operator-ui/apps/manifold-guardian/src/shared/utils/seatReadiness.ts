import type { ReadinessOutcome, ReadinessReport, UptimePermille } from '@operator-ui/types';

/** The checks one daemon readiness run makes, in display order. */
export type ReadinessCheck = 'relay' | 'discovery' | 'bitcoin' | 'version' | 'load' | 'uptime';

export const CHECKS: readonly ReadinessCheck[] = [
  'relay',
  'discovery',
  'bitcoin',
  'version',
  'load',
  'uptime'
];

export const labels: Record<ReadinessCheck, string> = {
  relay: 'Relay connection',
  discovery: 'Guardian discovery',
  bitcoin: 'Bitcoin backend',
  version: 'Software version',
  load: 'Host load',
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
  host_load_high: 'Above the limit for new seats.',
  uptime_1d_low: 'This host was offline too much recently.',
  uptime_7d_low: 'This host was offline too much recently.',
  uptime_30d_low: 'This host was offline too much recently.'
};

/** What a check that does not apply means for that check. */
const notApplicable: Partial<Record<ReadinessCheck, string>> = {
  version: 'No minimum set.',
  load: 'No limit set.'
};

/** The host-facing words for one check's outcome. */
export const describeOutcome = (check: ReadinessCheck, outcome: ReadinessOutcome): string =>
  (outcome === 'not_applicable' && notApplicable[check]) || outcomes[outcome];

/** What to do about a failing check, shown under it. The version and load checks have a one-line
 * outcome like the others; what to do about them is in the not-ready banner ({@link updateGuidance},
 * {@link loadGuidance}). */
export const guidance: Record<Exclude<ReadinessCheck, 'version' | 'load'>, string> = {
  relay:
    'Check that this host can hold long-lived outbound connections to the relay. No inbound port is needed.',
  discovery:
    'Check outbound DNS and HTTPS from this host. A new record can take a minute to publish after a relay change.',
  bitcoin:
    'Check the configured Bitcoin backend, its credentials, and its network. Let initial sync finish; do not expose RPC publicly.',
  uptime: 'Keep this host online.'
};

/** The host load figures the load check compared, e.g. "6-hour average 85%, limit 70%.", or
 * nothing where the daemon could not read the load or predates the check. */
export const describeHostLoad = (report: ReadinessReport): string | null => {
  const average = report.host_load_percent;
  if (average === null || average === undefined) return null;
  const limit = report.max_host_load_percent;
  return limit === null || limit === undefined
    ? `6-hour average ${average}%.`
    : `6-hour average ${average}%, limit ${limit}%.`;
};

export const updateGuidance =
  'This version of Manifold Guardian is below the minimum for new seats. Update to the latest release; this does not count against uptime.';

export const loadGuidance =
  'This host has been too busy for more seats. Move other work off it, or add CPU or faster disks; this does not count against uptime.';

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
