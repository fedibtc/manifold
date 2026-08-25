// Deterministic timestamp helpers for the Overview hub. `now` is always passed
// in (unix seconds) so every consumer is pure and unit-testable.

import type { Timestamp } from '@operator-ui/types';
import { timestampToDate } from '@/shared/utils/format';

// Resolve a wire Timestamp (Unix seconds) to unix seconds via the shared
// codec. Returns null when the value is absent.
export const parseTimestamp = (ts: Timestamp | null | undefined): number | null => {
  if (ts == null) return null;
  return Math.floor(timestampToDate(ts).getTime() / 1000);
};

// Human age for a non-negative second count: "just now", "42s ago", "5m ago",
// "3h ago", "2d ago".
export const formatAge = (seconds: number): string => {
  const s = Math.max(0, Math.floor(seconds));
  if (s < 5) return 'just now';
  if (s < 60) return `${s}s ago`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  return `${d}d ago`;
};

// Relative label for a wire timestamp against `now` (unix seconds). "—" when
// the timestamp is missing.
export const formatRelative = (ts: Timestamp | null | undefined, now: number): string => {
  const parsed = parseTimestamp(ts);
  if (parsed == null) return '—';
  return formatAge(now - parsed);
};
