import type { UptimePermille } from '@operator-ui/types';

/** Time constants of the daemon's uptime moving averages, in days. */
const TIME_CONSTANT_DAYS: Record<keyof UptimePermille, number> = { '1d': 1, '7d': 7, '30d': 30 };

/**
 * Days of unbroken uptime until `score` reaches `minimum` (both per-mille), or 0 when it passes.
 * The daemon updates s ← s·e^(−Δ/τ) + (1 − e^(−Δ/τ)) per up slot, so 1 − s decays as e^(−t/τ),
 * and it compares at 0.1%, so the target is half a step under the minimum.
 */
export const daysToRecover = (window: keyof UptimePermille, score: number, minimum: number) => {
  if (score >= minimum) return 0;
  const target = (minimum - 0.5) / 1000;
  return TIME_CONSTANT_DAYS[window] * Math.log((1 - score / 1000) / (1 - target));
};

export const formatDuration = (days: number): string => {
  const hours = days * 24;
  if (hours < 1) return 'less than an hour';
  if (hours < 36) return `about ${Math.round(hours)} ${Math.round(hours) === 1 ? 'hour' : 'hours'}`;
  return `about ${Math.round(days)} days`;
};
