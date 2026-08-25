import { deriveEarnings, type EarningsDay } from '@/features/overview/utils/deriveEarnings';
import { useGuardianFees } from '@/shared/api/hooks/use-guardian-fees/useGuardianFees';
import { usePaymentFederations } from '@/shared/api/hooks/use-payment-federations/usePaymentFederations';
import { useSeats } from '@/shared/api/hooks/use-seats/useSeats';
import { readTotalBalanceMsat } from '@/shared/utils/federationBalance';

export interface OverviewEarnings {
  /** Money figures are `null` when the daemon has not reported them yet, or
   *  could not — never a stand-in zero. The Overview renders those as "—". */
  balanceMsat: number | null;
  totalMsat: number | null;
  seatSalesMsat: number | null;
  guardianFeesMsat: number | null;
  days: EarningsDay[];
  /** Seats whose fee account could not be read at all — stated rather than
   *  folded into the totals as a zero. */
  unreadableFeeSeatCount: number;
  isLoading: boolean;
  /** True only when a fleet-wide query (seats or payment federations) failed —
   *  a per-seat fee lookup failing is already handled as unreadableFeeSeatCount. */
  isError: boolean;
  error: unknown;
}

export const useOverviewEarnings = (): OverviewEarnings => {
  const seats = useSeats();
  const paymentFederations = usePaymentFederations();
  const allSeats = seats.data?.seats ?? [];
  const liveSeats = allSeats.filter((seat) => !seat.decommissioned);
  const feeQueries = useGuardianFees(liveSeats.map((seat) => seat.seat_id));

  const guardianFees = feeQueries.flatMap((query) => (query.data ? [query.data] : []));
  const derived = deriveEarnings({ seats: allSeats, guardianFees });

  // A fee query that is still in flight has not reported "no fees" — it has
  // reported nothing. Summing it as zero states a total the fleet never earned,
  // so the fee-bearing figures stay unknown until every seat has answered. The
  // same holds when every seat's lookup failed: there is no partial truth left
  // to show, only an invented zero.
  const unreadableFeeSeatCount = feeQueries.filter((query) => query.isError).length;
  const feesPending = feeQueries.some((query) => query.isPending);
  const allFeesUnreadable = feeQueries.length > 0 && unreadableFeeSeatCount === feeQueries.length;
  const feesUnknown = feesPending || allFeesUnreadable;

  // Shared with the Wallet screen, so the two money screens state the same total
  // for the same wallets — including when one of them could not be read.
  const federations = paymentFederations.data?.federations;
  const balanceMsat = federations ? readTotalBalanceMsat(federations) : null;

  const seatSalesMsat = seats.data ? derived.seatSalesMsat : null;
  const guardianFeesMsat = feesUnknown ? null : derived.guardianFeesMsat;

  return {
    balanceMsat,
    seatSalesMsat,
    guardianFeesMsat,
    totalMsat:
      seatSalesMsat === null || guardianFeesMsat === null ? null : seatSalesMsat + guardianFeesMsat,
    days: derived.days,
    unreadableFeeSeatCount,
    isLoading: seats.isLoading || paymentFederations.isLoading,
    isError: seats.isError || paymentFederations.isError,
    error: seats.error ?? paymentFederations.error
  };
};
