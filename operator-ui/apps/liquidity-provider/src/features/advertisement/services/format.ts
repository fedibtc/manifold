import { truncateMiddle } from '@operator-ui/common-ui';
import type { GetAdvertisementStateResponse, SourceType, Timestamp } from '@operator-ui/types';
import { formatDateTime, timestampToDate } from '@/shared/utils/format';

const SOURCE_LABELS: Record<SourceType, string> = {
  gateway: 'Gateway (Lightning)',
  stability_pool: 'Stability pool'
};

// Coarse relative time ("2h ago" / "in 20h"). `now` is injected so callers stay
// testable; the mock server never reads a wall clock either.
export const formatRelative = (ts: Timestamp | null, now: number): string => {
  if (ts == null) return '—';
  const then = timestampToDate(ts).getTime();

  const diff = then - now;
  const abs = Math.abs(diff);
  const unit =
    abs < 3_600_000
      ? `${Math.max(1, Math.round(abs / 60_000))}m`
      : abs < 86_400_000
        ? `${Math.round(abs / 3_600_000)}h`
        : `${Math.round(abs / 86_400_000)}d`;

  return diff >= 0 ? `in ${unit}` : `${unit} ago`;
};

export const sourcesLabel = (sources: SourceType[]): string =>
  sources.length ? sources.map((source) => SOURCE_LABELS[source]).join(' · ') : '—';

export interface AdvertisementView {
  provider: string;
  endpoint: string;
  sources: string;
  lastPublished: string;
  expires: string;
  isWithdrawn: boolean;
  // Absolute UTC time the operator withdrew, when they still are withdrawn.
  // Absolute rather than relative: this is a decision the operator made and may
  // need to account for, not a freshness indicator.
  withdrawnAt: string | null;
}

// Display model for the advertisement screen: the opaque ids/endpoint truncated,
// the source list labelled, and the timestamps rendered relative to `now`.
export const deriveAdvertisement = (
  data: GetAdvertisementStateResponse,
  now: number
): AdvertisementView => {
  const ad = data.advertisement?.payload ?? null;
  return {
    provider: ad ? truncateMiddle(ad.provider_pubkey, 10, 6) : '—',
    endpoint: ad?.api_endpoints[0] ? truncateMiddle(ad.api_endpoints[0], 10, 6) : '—',
    sources: ad ? sourcesLabel(ad.supported_sources) : '—',
    lastPublished: formatRelative(data.last_published_at, now),
    expires: formatRelative(data.expires_at, now),
    // `withdrawn_at`, not the publication status. The status is a report of the
    // last thing the publisher did and the publisher moves on; `withdrawn_at`
    // is the operator's standing decision, and it is what the daemon itself
    // reads to stay off the relays.
    isWithdrawn: data.withdrawn_at !== null,
    withdrawnAt: data.withdrawn_at === null ? null : formatDateTime(data.withdrawn_at)
  };
};
