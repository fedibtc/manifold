// Pure Overview-hub derivation. Given the aggregated section snapshots (each of
// which may be undefined while its query loads), compute the status sentence,
// attention list, four metric tiles, recent-activity rows and the "updated"
// stamp. No React, no data fetching — this is the unit-tested core.

import type {
  AdminAllocationSummary,
  AdvertisementPublicationStatus,
  GetAdvertisementStateResponse,
  GetFundsResponse,
  GetHealthResponse,
  GetSetupStateResponse,
  HealthStatus,
  ReplenishmentStatus,
  Timestamp,
  WalletOperationSummary
} from '@operator-ui/types';
import { formatAge, parseTimestamp } from '@/features/overview/utils/time';
import { summaryStatus } from '@/shared/utils/allocationStatus';
import { formatSats, humanizeToken } from '@/shared/utils/format';

export interface OverviewInput {
  setup?: GetSetupStateResponse;
  funds?: GetFundsResponse;
  advertisement?: GetAdvertisementStateResponse;
  allocations?: AdminAllocationSummary[];
  walletOperations?: WalletOperationSummary[];
  health?: GetHealthResponse;
  now: number; // unix seconds
}

export type Severity = 'critical' | 'warning';

export interface AttentionAction {
  label: string;
  path: string;
}

export interface AttentionItem {
  key: string;
  severity: Severity;
  title: string;
  detail: string;
  action?: AttentionAction;
}

export interface OverviewStatus {
  tone: HealthStatus;
  headline: string;
  subline: string;
}

export type OverviewChipTone = 'ok' | 'warn' | 'bad' | 'info' | 'neutral';

export interface OverviewTile {
  key: string;
  label: string;
  value: string;
  hint: string | null;
  status: HealthStatus | null;
  chipTone: OverviewChipTone | null;
  loading: boolean;
}

export interface ActivityRow {
  key: string;
  when: string;
  event: string;
  amount: string;
  status: string;
}

export interface OverviewModel {
  status: OverviewStatus;
  updatedLabel: string | null;
  attention: AttentionItem[];
  tiles: OverviewTile[];
  activity: ActivityRow[];
}

const RECENT_ACTIVITY_LIMIT = 5;

const replenishmentTone: Record<ReplenishmentStatus, HealthStatus> = {
  ok: 'healthy',
  warning: 'warning',
  critical: 'unhealthy'
};

const replenishmentHint: Record<ReplenishmentStatus, string> = {
  ok: 'Healthy',
  warning: 'Running low',
  critical: 'Critically low'
};

const publicationLabels: Record<AdvertisementPublicationStatus, string> = {
  not_ready: 'Not ready',
  published: 'Published',
  stale: 'Stale',
  withdrawn: 'Withdrawn',
  failed: 'Failed'
};

const publicationTone: Record<AdvertisementPublicationStatus, HealthStatus> = {
  not_ready: 'warning',
  published: 'healthy',
  stale: 'warning',
  withdrawn: 'warning',
  failed: 'unhealthy'
};

const healthLabels: Record<HealthStatus, string> = {
  healthy: 'Healthy',
  warning: 'Warning',
  unhealthy: 'Unhealthy',
  unknown: 'Unknown'
};

const statusChipTone: Record<HealthStatus, OverviewChipTone> = {
  healthy: 'ok',
  warning: 'warn',
  unhealthy: 'bad',
  unknown: 'neutral'
};

const ADVERTISEMENT_ATTENTION: Partial<
  Record<AdvertisementPublicationStatus, Omit<AttentionItem, 'key' | 'action'>>
> = {
  failed: {
    severity: 'critical',
    title: 'Advertisement failed to publish',
    detail: 'Republish so initiators can discover this provider.'
  },
  not_ready: {
    severity: 'warning',
    title: 'Advertisement not ready',
    detail: 'Finish setup to publish an advertisement.'
  },
  stale: {
    severity: 'warning',
    title: 'Advertisement is stale',
    detail: 'Republish to refresh the listing before it expires.'
  },
  withdrawn: {
    severity: 'warning',
    title: 'Advertisement withdrawn',
    detail: 'The provider is not currently discoverable.'
  }
};

const deriveAttention = (input: OverviewInput): AttentionItem[] => {
  const items: AttentionItem[] = [];
  const { setup, funds, advertisement, allocations, health } = input;

  // Every non-ready status raises the setup gate, so the Overview is normally
  // unreachable while this holds; the item stays as a backstop for a status
  // that renders the shell without being ready. It points at Settings, which
  // is where configuration lives once the one-shot wizard is behind you —
  // there is no /setup route to send anyone to.
  if (setup && setup.status !== 'ready') {
    items.push({
      key: 'setup',
      severity: 'critical',
      title: 'Finish setup',
      detail: 'The service is not fully configured yet.',
      action: { label: 'Open settings', path: '/settings' }
    });
  }

  if (funds && funds.replenishment !== 'ok') {
    const critical = funds.replenishment === 'critical';
    items.push({
      key: 'funds',
      severity: critical ? 'critical' : 'warning',
      title: critical ? 'Available balance critically low' : 'Available balance running low',
      detail: critical
        ? 'Top up funds to keep serving allocations.'
        : 'Consider topping up soon to stay above the warning threshold.',
      action: { label: 'Review funds', path: '/funds' }
    });
  }

  if (advertisement) {
    const template = ADVERTISEMENT_ATTENTION[advertisement.publication_status];
    if (template) {
      items.push({
        key: 'advertisement',
        ...template,
        action: { label: 'Open advertisement', path: '/advertisement' }
      });
    }
  }

  if (allocations) {
    const failed = allocations.filter((a) => summaryStatus(a) === 'failed').length;
    if (failed > 0) {
      items.push({
        key: 'allocations',
        severity: 'critical',
        title: `${failed} allocation${failed === 1 ? '' : 's'} failed`,
        detail: 'Investigate the failed allocations and retry if appropriate.',
        action: { label: 'Review allocations', path: '/allocations' }
      });
    }
  }

  if (health && health.overall_status !== 'healthy') {
    const unhealthy = health.components.filter((c) => c.status !== 'healthy');
    const names = unhealthy.map((c) => humanizeToken(c.component)).join(', ');
    items.push({
      key: 'health',
      severity: health.overall_status === 'unhealthy' ? 'critical' : 'warning',
      title: 'System components degraded',
      detail: names ? `Affected: ${names}.` : 'One or more components are unhealthy.'
    });
  }

  return items;
};

const deriveStatus = (input: OverviewInput, attention: AttentionItem[]): OverviewStatus => {
  const anyLoaded = [
    input.setup,
    input.funds,
    input.advertisement,
    input.allocations,
    input.health
  ].some((value) => value !== undefined);

  if (!anyLoaded) {
    return {
      tone: 'healthy',
      headline: 'Checking system status…',
      subline: 'Loading the latest snapshot.'
    };
  }

  const hasCritical = attention.some((a) => a.severity === 'critical');
  const hasWarning = attention.some((a) => a.severity === 'warning');

  if (hasCritical) {
    return {
      tone: 'unhealthy',
      headline: 'Action required',
      subline: attentionSubline(attention.length)
    };
  }
  if (hasWarning) {
    return {
      tone: 'warning',
      headline: 'Attention recommended',
      subline: attentionSubline(attention.length)
    };
  }
  return {
    tone: 'healthy',
    headline: 'All systems operational',
    subline: 'The liquidity provider is healthy and discoverable.'
  };
};

const attentionSubline = (count: number): string =>
  `${count} item${count === 1 ? '' : 's'} need${count === 1 ? 's' : ''} your attention.`;

const deriveTiles = (input: OverviewInput): OverviewTile[] => {
  const { funds, advertisement, allocations, health } = input;

  const balanceTile: OverviewTile = funds
    ? {
        key: 'balance',
        label: 'Available balance',
        value: formatSats(funds.balance.available_balance),
        hint: replenishmentHint[funds.replenishment],
        status: replenishmentTone[funds.replenishment],
        chipTone: statusChipTone[replenishmentTone[funds.replenishment]],
        loading: false
      }
    : loadingTile('balance', 'Available balance');

  const adTile: OverviewTile = advertisement
    ? {
        key: 'advertisement',
        label: 'Advertisement',
        value: publicationLabels[advertisement.publication_status],
        hint: relayHint(advertisement),
        status: publicationTone[advertisement.publication_status],
        chipTone: statusChipTone[publicationTone[advertisement.publication_status]],
        loading: false
      }
    : loadingTile('advertisement', 'Advertisement');

  const allocationsTile: OverviewTile = allocations
    ? allocationTile(allocations)
    : loadingTile('allocations', 'Allocations');

  const healthTile: OverviewTile = health
    ? {
        key: 'health',
        label: 'Health',
        value: healthLabels[health.overall_status],
        hint: healthHint(health),
        status: health.overall_status,
        chipTone: statusChipTone[health.overall_status],
        loading: false
      }
    : loadingTile('health', 'Health');

  return [balanceTile, adTile, allocationsTile, healthTile];
};

const loadingTile = (key: string, label: string): OverviewTile => ({
  key,
  label,
  value: '—',
  hint: null,
  status: null,
  chipTone: null,
  loading: true
});

const relayHint = (advertisement: GetAdvertisementStateResponse): string => {
  const relays = advertisement.relay_states;
  const live = relays.filter((r) => r.status === 'published' || r.status === 'connected').length;
  return `${live}/${relays.length} relays live`;
};

const allocationTile = (allocations: AdminAllocationSummary[]): OverviewTile => {
  const failed = allocations.filter((a) => summaryStatus(a) === 'failed').length;
  const active = allocations.filter((a) => {
    const status = summaryStatus(a);
    return status === 'pending' || status === 'running';
  }).length;
  const hint =
    failed > 0 ? `${failed} failed` : active > 0 ? `${active} in progress` : 'None in progress';
  const status: HealthStatus = failed > 0 ? 'unhealthy' : 'healthy';
  const chipTone: OverviewChipTone = failed > 0 ? 'bad' : active > 0 ? 'info' : 'ok';
  return {
    key: 'allocations',
    label: 'Allocations',
    value: String(allocations.length),
    hint,
    status,
    chipTone,
    loading: false
  };
};

const healthHint = (health: GetHealthResponse): string => {
  const total = health.components.length;
  const healthy = health.components.filter((c) => c.status === 'healthy').length;
  return `${healthy}/${total} components healthy`;
};

const deriveActivity = (input: OverviewInput): ActivityRow[] => {
  const operations = input.walletOperations ?? [];
  return operations.slice(0, RECENT_ACTIVITY_LIMIT).map((op) => ({
    key: op.operation_id,
    when: formatRelativeWhen(op.created_at, input.now),
    event: humanizeToken(op.operation_type),
    amount: formatSats(op.amount),
    status: humanizeToken(op.status)
  }));
};

// Local wrapper kept separate from time.formatRelative so a future activity
// timestamp source can override formatting without touching the util.
const formatRelativeWhen = (ts: Timestamp, now: number): string => {
  const parsed = parseTimestamp(ts);
  return parsed == null ? '—' : formatAge(now - parsed);
};

const deriveUpdatedLabel = (input: OverviewInput): string | null => {
  const stamps = [
    input.health?.observed_at,
    input.funds?.gateway.observed_at,
    input.funds?.stability_pool.observed_at
  ]
    .map((ts) => parseTimestamp(ts))
    .filter((value): value is number => value != null);

  if (stamps.length === 0) return null;
  const freshest = Math.max(...stamps);
  return `Updated ${formatAge(input.now - freshest)}`;
};

export const deriveOverview = (input: OverviewInput): OverviewModel => {
  const attention = deriveAttention(input);
  return {
    status: deriveStatus(input, attention),
    updatedLabel: deriveUpdatedLabel(input),
    attention,
    tiles: deriveTiles(input),
    activity: deriveActivity(input)
  };
};
