import type { PaymentFederation, Plan } from '@operator-ui/types';
import { walletStatus } from '@/mocks/wallet-status';
import { deriveOverview } from '../deriveOverview';

const federation = (overrides: Partial<PaymentFederation> = {}): PaymentFederation => ({
  federation_id: 'fed1',
  accepted: true,
  receivable: true,
  ...overrides,
  wallet: overrides.wallet ?? walletStatus(5_000)
});

const PAID_OFFER: Plan[] = [{ InfiniteBestEffort: { price_msats: 50_000_000 } }];

it('should report success when every accepted federation is receivable', () => {
  const model = deriveOverview({ paymentFederations: [federation()], plans: PAID_OFFER });

  expect(model.tone).toBe('success');
  expect(model.headline).toBe('Advertised and healthy');
  expect(model.attention).toEqual([]);
});

it('should flag a non-receivable federation as an attention item linking to wallet', () => {
  const model = deriveOverview({
    paymentFederations: [federation({ receivable: false, wallet: walletStatus(0) })]
  });

  expect(model.tone).toBe('warn');
  expect(model.attention).toContainEqual({
    key: 'fed1',
    title: 'Payment federation not receiving',
    detail: 'fed1',
    path: '/wallet'
  });
});

it('should not flag a former member that stopped receiving', () => {
  const model = deriveOverview({
    paymentFederations: [federation({ accepted: false, receivable: false })]
  });

  expect(model.attention.map((item) => item.key)).not.toContain('fed1');
});

it('should warn when a paid offer has no payment federation at all', () => {
  const model = deriveOverview({ paymentFederations: [], plans: PAID_OFFER });

  expect(model.tone).toBe('warn');
  expect(model.attention).toContainEqual({
    key: 'no-receivable-payment-federation',
    title: 'Seats are priced but cannot be paid for',
    detail: 'No accepted payment federation can receive, so no seat can be bought.',
    path: '/offer'
  });
});

it('should warn when a paid offer has only a non-receivable federation', () => {
  const model = deriveOverview({
    paymentFederations: [federation({ receivable: false })],
    plans: PAID_OFFER
  });

  expect(model.attention.map((item) => item.key)).toContain('no-receivable-payment-federation');
});

it('should not warn about availability when the fleet is not selling', () => {
  const model = deriveOverview({ paymentFederations: [], plans: [] });

  expect(model.tone).toBe('success');
  expect(model.attention).toEqual([]);
});

it('should not warn about availability when seats are offered free', () => {
  const model = deriveOverview({
    paymentFederations: [],
    plans: [{ InfiniteBestEffort: { price_msats: 0 } }]
  });

  expect(model.attention).toEqual([]);
});

it('should handle no data with an empty, all-clear model', () => {
  const model = deriveOverview({});

  expect(model.tone).toBe('success');
  expect(model.attention).toEqual([]);
});

it('should raise an attention item when the authorization has not been observed', () => {
  const model = deriveOverview({ nostrState: 'not_observed' });

  const item = model.attention.find((entry) => entry.key === 'authorization-not-observed');
  expect(item?.title).toBe('No holder has authorized this fleet');
  expect(item?.path).toBe('/authorization');
});

// The daemon used to answer one state for both "nobody has authorized this" and
// "the relay has not been read", so this item could only report what was not
// known. `not_observed` is a completed read, so the item says what is true.
it('should state plainly that no holder has authorized the fleet', () => {
  const model = deriveOverview({ nostrState: 'not_observed' });

  const item = model.attention.find((entry) => entry.key === 'authorization-not-observed');
  expect(item?.detail).toMatch(/Open Authorization/i);
  expect(item?.detail).not.toMatch(/still be reading/i);
});

// `checking` clears on its own within one relay read of daemon start. An item
// the operator cannot act on is noise.
it('should raise nothing while the first relay read is still outstanding', () => {
  const model = deriveOverview({ nostrState: 'checking' });

  expect(model.attention.map((entry) => entry.key)).not.toContain('authorization-not-observed');
});

// A failed read and an absent authorization are different facts. Folding them
// together would tell an operator with a relay outage that no holder had signed.
it('should raise a distinct item when the relay could not be read', () => {
  const model = deriveOverview({ nostrState: 'relay_error' });

  const keys = model.attention.map((entry) => entry.key);
  expect(keys).toContain('authorization-relay-error');
  expect(keys).not.toContain('authorization-not-observed');
});

it('should raise nothing once the authorization is observed', () => {
  const model = deriveOverview({ nostrState: 'authorization_observed' });

  expect(model.attention).toHaveLength(0);
});

it('should raise nothing when the state is unknown', () => {
  const model = deriveOverview({});

  expect(model.attention).toHaveLength(0);
});
