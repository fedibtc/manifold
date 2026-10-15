import type { ReadinessOutcome, ReadinessReport } from '@operator-ui/types';

/** The checks one daemon readiness run makes, in display order. */
export type ReadinessCheck = 'relay' | 'discovery' | 'bitcoin' | 'uptime';

export const CHECKS: readonly ReadinessCheck[] = ['relay', 'discovery', 'bitcoin', 'uptime'];

export const labels: Record<ReadinessCheck, string> = {
  relay: 'Relay connection',
  discovery: 'Guardian discovery',
  bitcoin: 'Bitcoin backend',
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
  uptime_1d_low: 'This host was online less of the past day than Fedi requires (70% by default).',
  uptime_7d_low: 'This host was online less of the past week than Fedi requires (90% by default).',
  uptime_30d_low: 'This host was online less of the past month than Fedi requires (93% by default).'
};

export const guidance: Record<ReadinessCheck, string> = {
  relay:
    'Check that this host can hold long-lived outbound connections to the relay. No inbound port is needed.',
  discovery:
    'Check outbound DNS and HTTPS from this host. A new record can take a minute to publish after a relay change.',
  bitcoin:
    'Check the configured Bitcoin backend, its credentials, and its network. Let initial sync finish; do not expose RPC publicly.',
  uptime:
    'Keep this host and its checks running. New seats resume once uptime is back above the minimum: some hours after a long outage, about two days after a one-day outage.'
};

export const passed = (outcome: ReadinessOutcome): boolean =>
  outcome === 'pass' || outcome === 'not_applicable';

export const failedChecks = (report: ReadinessReport): ReadinessCheck[] =>
  CHECKS.filter((check) => !passed(report[check]));
