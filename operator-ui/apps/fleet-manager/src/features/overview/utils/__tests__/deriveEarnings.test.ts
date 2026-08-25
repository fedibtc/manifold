import type { GuardianFeesResponse, SeatSummary } from '@operator-ui/types';
import { walletStatus } from '@/mocks/wallet-status';
import { deriveEarnings } from '../deriveEarnings';

const DAY_ONE = Date.UTC(2026, 7, 3, 12);
const DAY_TWO = Date.UTC(2026, 7, 4, 9);

const seat = (overrides: Partial<SeatSummary> & Pick<SeatSummary, 'seat_id'>): SeatSummary => ({
  fi_id: 'fi_1',
  plan: { InfiniteBestEffort: { price_msats: 50_000_000 } },
  created_at_ms: DAY_ONE,
  payment_claim: { state: 'success', at_ms: DAY_ONE },
  completion_callback: { state: 'not_configured' },
  decommissioned: false,
  backup: null,
  ...overrides
});

const windowSum = (remittances: GuardianFeesResponse['remittances']): number =>
  remittances.reduce((total, one) => total + one.amount_msat, 0);

// `lifetimeRemittedMsat` defaults to the window's own sum, which is what a seat
// young enough to fit inside the display window reports. Pass it explicitly to
// build the seat this module got wrong: one that has outgrown the window, whose
// lifetime figure is therefore larger than anything the list can show.
const fees = (
  seat_id: string,
  remittances: GuardianFeesResponse['remittances'],
  lifetimeRemittedMsat: number = windowSum(remittances)
): GuardianFeesResponse => ({
  seat_id,
  federation_id: 'fed1abc',
  remittance_account: '{}',
  lifetime_remitted_msat: lifetimeRemittedMsat,
  collectable_msat: 0,
  staged_msat: 0,
  locked_msat: 0,
  idle_msat: 0,
  wallet: walletStatus(0),
  policy: {
    configured: true,
    send_ppm: 1_000,
    recipients: null,
    share_matches_policy: true,
    our_weight: 1,
    total_weight: 4
  },
  remittances
});

it('should report zero across the board with no seats and no fees', () => {
  expect(deriveEarnings({})).toEqual({
    totalMsat: 0,
    seatSalesMsat: 0,
    guardianFeesMsat: 0,
    days: []
  });
});

it('should count a seat sale only once its payment claim succeeded', () => {
  const model = deriveEarnings({
    seats: [
      seat({ seat_id: 'sold' }),
      seat({ seat_id: 'pending', payment_claim: { state: 'pending' } }),
      seat({
        seat_id: 'already-spent',
        payment_claim: { state: 'already_spent', at_ms: DAY_ONE }
      })
    ]
  });

  expect(model.seatSalesMsat).toBe(50_000_000);
});

it('should price a seat sale from the plan the seat was sold under', () => {
  const model = deriveEarnings({
    seats: [seat({ seat_id: 'cheap', plan: { InfiniteBestEffort: { price_msats: 1_000 } } })]
  });

  expect(model.seatSalesMsat).toBe(1_000);
});

it('should add guardian-fee remittances to the total', () => {
  const model = deriveEarnings({
    seats: [seat({ seat_id: 'sold' })],
    guardianFees: [
      fees('sold', [
        { amount_msat: 4_000_000, txid: 'tx1', remitted_at_unix: Math.floor(DAY_TWO / 1000) }
      ])
    ]
  });

  expect(model.guardianFeesMsat).toBe(4_000_000);
  expect(model.totalMsat).toBe(54_000_000);
});

// The fault this replaces: the total was the sum of `remittances`, and the
// daemon caps that list at 20 entries per seat. A seat past its 21st payment
// reported a lifetime figure short by everything the window dropped — here,
// 16,000,000 msat shown against 41,500,000 msat actually remitted. The window
// and the lifetime figure are deliberately different numbers below; a total
// taken from the list cannot pass by accident.
it('should total guardian fees from the lifetime figure, not the returned window', () => {
  const at = Math.floor(DAY_TWO / 1000);
  const model = deriveEarnings({
    guardianFees: [
      fees(
        'outgrown',
        [
          { amount_msat: 6_000_000, txid: 'tx-newest', remitted_at_unix: at },
          { amount_msat: 4_000_000, txid: 'tx-middle', remitted_at_unix: at - 60 },
          { amount_msat: 6_000_000, txid: 'tx-oldest-shown', remitted_at_unix: at - 120 }
        ],
        41_500_000
      )
    ]
  });

  expect(model.guardianFeesMsat).toBe(41_500_000);
  expect(model.totalMsat).toBe(41_500_000);
  // The window is what the timeline shows, and it is smaller. Asserting it here
  // keeps the two figures pinned apart.
  expect(model.days).toHaveLength(1);
  expect(model.days[0].totalMsat).toBe(16_000_000);
});

it('should add each seat lifetime figure across the fleet', () => {
  const at = Math.floor(DAY_TWO / 1000);
  const model = deriveEarnings({
    guardianFees: [
      fees(
        'outgrown',
        [{ amount_msat: 6_000_000, txid: 'tx-a', remitted_at_unix: at }],
        41_500_000
      ),
      fees('young', [{ amount_msat: 2_500_000, txid: 'tx-b', remitted_at_unix: at }], 2_500_000)
    ]
  });

  expect(model.guardianFeesMsat).toBe(44_000_000);
});

it('should bucket events by day, newest first', () => {
  const model = deriveEarnings({
    seats: [seat({ seat_id: 'sold', payment_claim: { state: 'success', at_ms: DAY_ONE } })],
    guardianFees: [
      fees('sold', [
        { amount_msat: 4_000_000, txid: 'tx1', remitted_at_unix: Math.floor(DAY_TWO / 1000) }
      ])
    ]
  });

  expect(model.days.map((bucket) => bucket.day)).toEqual(['2026-08-04', '2026-08-03']);
  expect(model.days[0].totalMsat).toBe(4_000_000);
  expect(model.days[1].totalMsat).toBe(50_000_000);
});

it('should keep an undated remittance in the total and sort its bucket last', () => {
  const model = deriveEarnings({
    seats: [seat({ seat_id: 'sold' })],
    guardianFees: [
      fees('sold', [{ amount_msat: 7_000, txid: 'tx-sealed', breakdown_error: 'unreadable' }])
    ]
  });

  expect(model.guardianFeesMsat).toBe(7_000);
  expect(model.days.at(-1)?.day).toBe(null);
  expect(model.days.at(-1)?.totalMsat).toBe(7_000);
});

it('should sum several remittances on the same day into one bucket', () => {
  const at = Math.floor(DAY_TWO / 1000);
  const model = deriveEarnings({
    guardianFees: [
      fees('sold', [
        { amount_msat: 1_000, txid: 'tx1', remitted_at_unix: at },
        { amount_msat: 2_000, txid: 'tx2', remitted_at_unix: at + 60 }
      ])
    ]
  });

  expect(model.days).toHaveLength(1);
  expect(model.days[0].totalMsat).toBe(3_000);
  expect(model.days[0].events).toHaveLength(2);
});
