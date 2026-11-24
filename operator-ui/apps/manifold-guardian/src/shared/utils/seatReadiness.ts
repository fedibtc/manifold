import type {
  PressurePermille,
  ReadinessOutcome,
  ReadinessReport,
  UptimePermille
} from '@operator-ui/types';

/** The checks one daemon readiness run makes, in display order. */
export type ReadinessCheck =
  | 'relay'
  | 'discovery'
  | 'bitcoin'
  | 'version'
  | 'pressure'
  | 'ordering_latency'
  | 'uptime';

export const CHECKS: readonly ReadinessCheck[] = [
  'relay',
  'discovery',
  'bitcoin',
  'version',
  'pressure',
  'ordering_latency',
  'uptime'
];

export const labels: Record<ReadinessCheck, string> = {
  relay: 'Relay connection',
  discovery: 'Guardian discovery',
  bitcoin: 'Bitcoin backend',
  version: 'Software version',
  pressure: 'CPU, disk and memory pressure',
  ordering_latency: 'Consensus speed',
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
  host_pressure_6h_high: 'Too busy over the last 6 hours.',
  host_pressure_3_5d_high: 'Too busy over the last 3.5 days.',
  host_pressure_unavailable: 'This host’s CPU, disk and memory pressure could not be read.',
  host_pressure_measuring: 'Measuring. The first figures arrive a minute after the host starts.',
  ordering_latency_high: 'Too slow over the last 6 hours.',
  ordering_latency_measuring:
    'Measuring. The first figure arrives a few minutes after a guardian starts running.',
  uptime_1d_low: 'This host was offline too much recently.',
  uptime_7d_low: 'This host was offline too much recently.',
  uptime_30d_low: 'This host was offline too much recently.'
};

/** What a check that does not apply means for that check. */
const notApplicable: Partial<Record<ReadinessCheck, string>> = {
  version: 'No minimum set.',
  ordering_latency: 'No guardian is running yet.'
};

/** The host-facing words for one check's outcome. */
export const describeOutcome = (check: ReadinessCheck, outcome: ReadinessOutcome): string =>
  (outcome === 'not_applicable' && notApplicable[check]) || outcomes[outcome];

/** What to do about a failing check, shown under it. The version and consensus speed checks, and
 * a pressure average over its maximum, have a one-line outcome like the others; what to do about
 * them is in the not-ready banner ({@link updateGuidance}, {@link pressureGuidance},
 * {@link orderingLatencyGuidance}). */
export const guidance: Record<Exclude<ReadinessCheck, 'version' | 'ordering_latency'>, string> = {
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

export const updateGuidance =
  'This version of Manifold Guardian is below the minimum for new seats. Update to the latest release; this does not count against uptime.';

export const pressureGuidance =
  'This host has been too busy for more seats: its tasks spent too long waiting for CPU, disk or memory. Move other work off it, or add CPU, memory or faster disks; this does not count against uptime.';

export const orderingLatencyGuidance =
  'This host’s guardians have been slow to get their own consensus items ordered, which costs their federations margin. A slow or busy disk is the usual cause, often from other software on this host. Move disk-heavy work off it, or use a faster disk; this does not count against uptime.';

/** Whether a pressure outcome is an average over its maximum, rather than missing figures. */
export const pressureHigh = (outcome: ReadinessOutcome | undefined): boolean =>
  outcome === 'host_pressure_6h_high' || outcome === 'host_pressure_3_5d_high';

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

/** A millisecond figure as seconds, e.g. 6200 → "6.2 s". */
export const formatMillis = (millis: number): string => `${(millis / 1000).toFixed(1)} s`;

/** The consensus speed figure the gate compares. */
export const orderingLatencyLabel = 'Ordering time, 6-hour average';

/** The pressure resources the gate compares, as the operator reads them. */
export const PRESSURE_RESOURCES = ['cpu', 'io', 'memory'] as const;

/** The pressure windows the gate compares, shortest first. */
export const PRESSURE_WINDOWS: readonly (keyof PressurePermille)[] = ['6h', '3.5d'];

export const pressureLabels: Record<
  (typeof PRESSURE_RESOURCES)[number],
  Record<keyof PressurePermille, string>
> = {
  cpu: { '6h': 'CPU, 6-hour average', '3.5d': 'CPU, 3.5-day average' },
  io: { '6h': 'Disk, 6-hour average', '3.5d': 'Disk, 3.5-day average' },
  memory: { '6h': 'Memory, 6-hour average', '3.5d': 'Memory, 3.5-day average' }
};
