import { useGuardianFees } from '@/shared/api/hooks/use-guardian-fees/useGuardianFees';
import { useSeats } from '@/shared/api/hooks/use-seats/useSeats';

export interface GuardianFeeRow {
  seatId: string;
  /** Remitted and still in the pool. `null` when the seat's fee account has not
   *  answered, or could not be read — never a stand-in zero. */
  collectableMsat: number | null;
  /** Already collected out of the pool and sitting as ordinary ecash, which is
   *  the only money a guardian-fee sweep can send. `null` reads as unknown. */
  collectedEcashMsat: number | null;
}

/**
 * One row per live seat, because guardian-fee revenue is per seat and there is
 * no aggregate verb. A decommissioned seat is left out: it earns nothing new,
 * and its account is not what an operator comes to this screen to move.
 *
 * A seat whose fee account cannot be read still gets a row. Dropping it would
 * hide money; showing it with a zero would claim there is none. It shows "—" and
 * the operator can still press the buttons, because the daemon is the authority
 * on whether there is anything to take.
 */
export const useGuardianFeeRows = (): GuardianFeeRow[] => {
  const seats = useSeats();
  const liveSeats = (seats.data?.seats ?? []).filter((seat) => !seat.decommissioned);
  const feeQueries = useGuardianFees(liveSeats.map((seat) => seat.seat_id));

  return liveSeats.map((seat, index) => {
    const fees = feeQueries[index]?.data;
    return {
      seatId: seat.seat_id,
      collectableMsat: fees ? fees.collectable_msat : null,
      collectedEcashMsat: fees ? fees.wallet.available_ecash_msat : null
    };
  });
};
