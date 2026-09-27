import type {
  Check,
  CheckId,
  CheckStatus,
  ReasonCode,
  SelfCheckReport,
  SelfCheckResponse
} from '@operator-ui/types';

const labels: Record<CheckId, string> = {
  discovery_dns: 'Guardian discovery name (daemon resolver)',
  discovery_https: 'Guardian discovery HTTPS transport',
  fman_relay: 'FMan control-plane relay',
  bitcoin_dns: 'Bitcoin primary name (daemon resolver)',
  bitcoin_primary: 'Bitcoin primary API',
  bitcoin_fallback: 'Configured Bitcoin fallback API',
  guardian_health: 'Cached formed guardian observations',
  directory_observation: 'Retained directory enrollment observation'
};

const statuses: Record<CheckStatus, string> = {
  pass: 'Pass',
  warning: 'Warning',
  failure: 'Failure',
  unknown: 'Unknown',
  not_applicable: 'Not applicable'
};

const reasons: Record<ReasonCode, string> = {
  reached: 'The configured service answered this narrow check.',
  no_records: 'This name did not resolve during the check.',
  timeout: 'This attempt timed out; it may be transient.',
  unreachable: 'This attempt could not reach the configured service.',
  numeric_host: 'The configured target uses a numeric address; name resolution is not needed.',
  not_https: 'The configured service does not use HTTPS; TLS was not tested.',
  unsupported_configuration: 'This configuration cannot be checked safely by this version.',
  unsupported_proxy_configuration:
    'HTTP checks were skipped because the daemon has proxy settings that may differ from guardian processes.',
  run_deadline: 'The run ended before this observation completed.',
  connected: 'The running FMan endpoint has a connected control-plane relay.',
  disconnected: 'No connected control-plane relay was observed at this instant.',
  not_selected: 'No control-plane relay was selected at this instant.',
  disabled: 'Control-plane relays are disabled in this configuration.',
  http_access: 'The service answered but refused access.',
  http_service: 'The service answered with a temporary or server-side HTTP response.',
  invalid_response: 'The service response did not satisfy this narrow API check.',
  wrong_network: 'The Bitcoin service reports a different network.',
  synchronizing: 'Bitcoin Core reports initial block download in progress.',
  starting: 'Bitcoin Core reports it is starting.',
  request_rejected: 'The Bitcoin API rejected the read-only request.',
  not_configured: 'No explicit fallback is configured.',
  cached_healthy: 'Formed guardians were healthy in their last cached watchdog observations.',
  cached_unavailable: 'At least one formed guardian was unavailable in its cached observation.',
  no_formed_seats: 'No formed guardians are present to assess.',
  retained_authorization:
    'A verified authorization has been retained; current relay access was not checked.',
  checking: 'No directory observation has completed yet.',
  not_observed: 'The last directory read found no authorization.',
  previous_relay_error: 'A previous directory read failed; current relay access was not checked.'
};

const guidance: Record<CheckId, string> = {
  discovery_dns:
    'If unresolved, inspect the FMan host/container DNS settings and configured discovery service availability.',
  discovery_https:
    'If unreachable, inspect outbound access and the configured service. For TLS failures, inspect time, CA trust, and interception; do not disable certificate checks.',
  fman_relay:
    'If disconnected, inspect outbound long-lived relay access and relay availability. No inbound port is required.',
  bitcoin_dns:
    'If unresolved, inspect the configured Bitcoin service name from the FMan host/container.',
  bitcoin_primary:
    'If failing, inspect the configured Bitcoin API, RPC access/credentials, and intended network locally. Allow initial sync to finish; do not expose RPC publicly.',
  bitcoin_fallback:
    'If degraded, inspect only the operator-approved fallback and its network. A healthy fallback does not hide a broken primary.',
  guardian_health:
    'If unavailable, inspect the existing guardian seat view and process health locally; network checks may explain a transient outage.',
  directory_observation:
    'If not observed, check enrollment in the operator view. This retained observation does not verify current Nostr connectivity.'
};

const order = Object.keys(labels) as CheckId[];
const known = (table: object, key: unknown): key is keyof typeof table =>
  typeof key === 'string' && Object.hasOwn(table, key);
const record = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value);
const keys = (value: Record<string, unknown>, expected: string[]) =>
  Object.keys(value).length === expected.length &&
  expected.every((key) => Object.hasOwn(value, key));

// A type parameter on adminCall is a cast, not wire validation. Reject all extensions
// and unknown vocabulary before any value can reach preview or clipboard.
export const parseSelfCheckResponse = (value: unknown): SelfCheckResponse | null => {
  if (
    !record(value) ||
    !known({ completed: true, busy: true, cooldown: true, unavailable: true }, value.state)
  ) {
    return null;
  }
  if (value.state !== 'completed') {
    return keys(value, ['state']) ? (value as SelfCheckResponse) : null;
  }
  if (!keys(value, ['state', 'report']) || !record(value.report)) return null;
  const report = value.report;
  if (
    !keys(report, ['schema_version', 'checks']) ||
    report.schema_version !== 1 ||
    !Array.isArray(report.checks) ||
    report.checks.length !== order.length
  ) {
    return null;
  }
  for (const [index, entry] of report.checks.entries()) {
    if (
      !record(entry) ||
      !keys(entry, ['check_id', 'status', 'reason_code']) ||
      entry.check_id !== order[index] ||
      !known(statuses, entry.status) ||
      !known(reasons, entry.reason_code)
    ) {
      return null;
    }
  }
  return value as SelfCheckResponse;
};

export const formatSelfCheckReport = (report: SelfCheckReport): string => {
  const lines = [
    'FMan self-check',
    'Generic outcomes and guidance only; review before sharing.',
    ''
  ];
  for (const check of report.checks as Check[]) {
    lines.push(`${labels[check.check_id]} — ${statuses[check.status]}`);
    lines.push(`  ${reasons[check.reason_code]}`);
    if (check.status === 'warning' || check.status === 'failure' || check.status === 'unknown') {
      lines.push(`  What to check: ${guidance[check.check_id]}`);
    }
  }
  lines.push(
    '',
    'Limits: These are daemon-side samples and cached observations, not proof of guardian discovery, direct UDP, remote peers, consensus, chain-tip freshness, current enrollment or missing storage/data. No automatic changes or report upload occurred.'
  );
  return lines.join('\n');
};
