import type { ReadinessOutcome, ReadinessReport, UptimePermille } from '@operator-ui/types';

/** The checks one daemon readiness run makes, in display order. */
export type ReadinessCheck = 'relay' | 'discovery' | 'bitcoin' | 'seats' | 'uptime';

export const CHECKS: readonly ReadinessCheck[] = [
  'relay',
  'discovery',
  'bitcoin',
  'seats',
  'uptime'
];

export const labels: Record<ReadinessCheck, string> = {
  relay: 'Relay connection',
  discovery: 'Guardian discovery',
  bitcoin: 'Bitcoin backend',
  seats: 'Hosted seats',
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
  seats_not_serving:
    'Most of this host’s seats have not completed a consensus session with their federation recently.',
  uptime_1d_low: 'This host or its seats were down too much recently.',
  uptime_7d_low: 'This host or its seats were down too much recently.',
  uptime_30d_low: 'This host or its seats were down too much recently.'
};

/** What a check that does not apply means for that check. */
const notApplicable: Partial<Record<ReadinessCheck, string>> = {
  seats: 'This host has no formed seats to check.'
};

/** The operator-facing words for one check's outcome. */
export const describeOutcome = (check: ReadinessCheck, outcome: ReadinessOutcome): string =>
  (outcome === 'not_applicable' && notApplicable[check]) || outcomes[outcome];

export const guidance: Record<ReadinessCheck, string> = {
  relay:
    'Check that this host can hold long-lived outbound connections to the relay. No inbound port is needed.',
  discovery:
    'Check outbound DNS and HTTPS from this host. A new record can take a minute to publish after a relay change.',
  bitcoin:
    'Check the configured Bitcoin backend, its credentials, and its network. Let initial sync finish; do not expose RPC publicly.',
  seats:
    'Check the Seats page and the seat logs. A seat that keeps restarting or falls behind its federation often means this host is short of memory or network.',
  uptime: 'Keep this host online and its seats serving.'
};

export const passed = (outcome: ReadinessOutcome): boolean =>
  outcome === 'pass' || outcome === 'not_applicable';

export const failedChecks = (report: ReadinessReport): ReadinessCheck[] =>
  CHECKS.filter((check) => !passed(report[check]));

/** The uptime windows the gate compares, shortest first. */
export const UPTIME_WINDOWS: readonly (keyof UptimePermille)[] = ['1d', '7d', '30d'];

export const uptimeLabels: Record<keyof UptimePermille, string> = {
  '1d': 'Last day',
  '7d': 'Last 7 days',
  '30d': 'Last 30 days'
};

/** A per-mille figure as the percent the gate compares, e.g. 899 → "89.9%". */
export const formatPermille = (permille: number): string => `${(permille / 10).toFixed(1)}%`;
