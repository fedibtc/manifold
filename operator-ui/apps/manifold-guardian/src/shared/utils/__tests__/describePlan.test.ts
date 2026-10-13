import { describePlan } from '../describePlan';

it('should describe a paid one-time plan in sats', () => {
  expect(describePlan({ InfiniteBestEffort: { price_msats: 50_000_000 } })).toBe(
    '50,000 sats, one-time'
  );
});

it('should describe a zero-price plan as free', () => {
  expect(describePlan({ InfiniteBestEffort: { price_msats: 0 } })).toBe('Free, one-time');
});

it('should describe SubscriptionBased as unavailable in v1', () => {
  expect(
    describePlan({
      SubscriptionBased: {
        initial_price_msats: 1_000,
        renewal_price_msats: 1_000,
        period: 'monthly'
      }
    })
  ).toBe('Subscription (unavailable in v1)');
});
