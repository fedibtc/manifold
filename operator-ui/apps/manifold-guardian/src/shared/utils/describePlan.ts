import type { Plan } from '@operator-ui/types';

// SubscriptionBased exists in the Plan enum but the v1 daemon refuses to offer it
// — still handled here defensively, since ShowPlans's return type is Vec<Plan> and
// a seat sold under a stale offer could in principle carry one.
export const describePlan = (plan: Plan): string => {
  if ('InfiniteBestEffort' in plan) {
    const { price_msats } = plan.InfiniteBestEffort;
    if (price_msats === 0) return 'Free, one-time';
    return `${Math.floor(price_msats / 1000).toLocaleString('en-US')} sats, one-time`;
  }
  return 'Subscription (unavailable in v1)';
};
