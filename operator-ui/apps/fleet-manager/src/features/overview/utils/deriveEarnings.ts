import type { GuardianFeesResponse, SeatSummary } from '@operator-ui/types';

export type EarningKind = 'seat-sale' | 'guardian-fee';

export interface EarningEvent {
  key: string;
  kind: EarningKind;
  amountMsat: number;
  detail: string;
  /** Null for a remittance whose sealed breakdown would not open: the money is
   *  still ours, but the payer's timestamp came with the paperwork. */
  atMs: number | null;
}

export interface EarningsDay {
  /** ISO date, or null for the undated bucket. */
  day: string | null;
  totalMsat: number;
  events: EarningEvent[];
}

export interface EarningsModel {
  totalMsat: number;
  seatSalesMsat: number;
  /** The daemon's own lifetime scalar per seat, added up across the fleet's
   *  seats. Never the sum of `days` — see `lifetimeRemitted` below. */
  guardianFeesMsat: number;
  /** Recent activity, for the timeline. Its fee half is the display window the
   *  daemon returned, so these buckets do not add up to `guardianFeesMsat` on a
   *  seat that has outgrown the window. */
  days: EarningsDay[];
}

export interface EarningsInputs {
  seats?: SeatSummary[];
  guardianFees?: GuardianFeesResponse[];
}

const planPriceMsat = (plan: SeatSummary['plan']): number =>
  'InfiniteBestEffort' in plan ? plan.InfiniteBestEffort.price_msats : 0;

const dayOf = (atMs: number | null): string | null =>
  atMs === null ? null : new Date(atMs).toISOString().slice(0, 10);

// A seat counts as sold at the moment its payment claim was accepted. That is an
// accepted claim, not a settlement — the caveat the Overview states on screen.
const seatSales = (seats: SeatSummary[]): EarningEvent[] =>
  seats
    .filter((seat) => seat.payment_claim.state === 'success')
    .map((seat) => ({
      key: `seat-sale:${seat.seat_id}`,
      kind: 'seat-sale' as const,
      amountMsat: planPriceMsat(seat.plan),
      detail: seat.seat_id,
      atMs: seat.payment_claim.state === 'success' ? seat.payment_claim.at_ms : null
    }));

// `remittances` is a display window: the daemon caps it (`limit.unwrap_or(20)`),
// so this is the seat's recent fee activity and nothing older. It feeds the
// timeline only — a money total taken from it is short by everything the window
// dropped, which is the whole of `lifetimeRemitted` below.
const feeRemittances = (guardianFees: GuardianFeesResponse[]): EarningEvent[] =>
  guardianFees.flatMap((fees) =>
    fees.remittances.map((remittance) => ({
      key: `guardian-fee:${remittance.txid}`,
      kind: 'guardian-fee' as const,
      amountMsat: remittance.amount_msat,
      detail: fees.seat_id,
      atMs: remittance.remitted_at_unix === undefined ? null : remittance.remitted_at_unix * 1000
    }))
  );

// Everything ever remitted to each seat, swept funds included. The daemon walks
// the seat's full account history for this (`fman_core::guardian_fee::
// total_remitted`) and reports it as a scalar, so a lifetime figure never
// depends on how many entries a page happened to carry. Adding across seats is
// not the same shape: `ListSeats` is the whole fleet, unpaginated.
const lifetimeRemitted = (guardianFees: GuardianFeesResponse[]): number =>
  guardianFees.reduce((total, fees) => total + fees.lifetime_remitted_msat, 0);

const sumMsat = (events: EarningEvent[]): number =>
  events.reduce((total, event) => total + event.amountMsat, 0);

// Newest day first; the undated bucket sorts last, since it cannot claim a place
// in the timeline.
const byDayDescending = (left: EarningsDay, right: EarningsDay): number => {
  if (left.day === null) return 1;
  if (right.day === null) return -1;
  return right.day.localeCompare(left.day);
};

const bucketByDay = (events: EarningEvent[]): EarningsDay[] => {
  const buckets = new Map<string | null, EarningEvent[]>();
  for (const event of events) {
    const day = dayOf(event.atMs);
    const bucket = buckets.get(day);
    if (bucket) bucket.push(event);
    else buckets.set(day, [event]);
  }

  return [...buckets.entries()]
    .map(([day, dayEvents]) => ({
      day,
      totalMsat: sumMsat(dayEvents),
      events: [...dayEvents].sort((left, right) => (right.atMs ?? 0) - (left.atMs ?? 0))
    }))
    .sort(byDayDescending);
};

/**
 * Both revenue streams on one timeline: seats sold, and guardian fees remitted
 * by the federations this fleet guards. Every figure is gross — the daemon
 * reports what it was paid, before mint and Lightning fees.
 *
 * The two money totals come from unwindowed sources, deliberately: seat sales
 * from the whole seat list, guardian fees from the daemon's lifetime scalar.
 * Neither is a sum of a paginated collection.
 */
export const deriveEarnings = ({
  seats = [],
  guardianFees = []
}: EarningsInputs): EarningsModel => {
  const sales = seatSales(seats);
  const recentFees = feeRemittances(guardianFees);
  const seatSalesMsat = sumMsat(sales);
  const guardianFeesMsat = lifetimeRemitted(guardianFees);

  return {
    totalMsat: seatSalesMsat + guardianFeesMsat,
    seatSalesMsat,
    guardianFeesMsat,
    days: bucketByDay([...sales, ...recentFees])
  };
};
