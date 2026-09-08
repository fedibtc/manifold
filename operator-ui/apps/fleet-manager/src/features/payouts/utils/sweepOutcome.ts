import type { CollectGuardianFeesResponse, PayoutJob } from '@operator-ui/types';
import { formatSats } from '@/shared/utils/format';

/** What a settled sweep moved. Both sweep verbs answer in this shape. */
export const describePayout = (payout: PayoutJob): string =>
  payout.operation === null
    ? 'Payout request is pending.'
    : `Sent ${formatSats(payout.operation.amount_msat)}.`;

/**
 * What a collection took, what it could not, and whether it finished.
 *
 * A collection reports what it *could* claim: locked deposits leave the pool
 * only at the next cycle turnover, so `awaiting_cycle_msat` is the part of the
 * account that is still there afterwards. Both figures are always stated —
 * naming only the claimed amount would read as "the account is now empty", which
 * is a different and often false claim.
 *
 * Two cases the daemon added with incomplete collections, and neither may be
 * flattened into the happy one:
 *
 * - The collection failed partway. `claimed_msat` then counts only value a
 *   terminal operation confirmed, so it is a floor, not a total, and the
 *   operator has to know to run it again.
 * - The post-failure balance read may itself have failed, leaving
 *   `awaiting_cycle_msat` null. That is *unknown*, not zero — `formatSats`
 *   renders it as `—`, and saying "0 sats are waiting" there would invent a
 *   fact about the operator's money.
 */
export const describeCollection = (collected: CollectGuardianFeesResponse): string => {
  const awaiting =
    collected.awaiting_cycle_msat === null
      ? `${formatSats(null)} is waiting for the next cycle turnover — the balance could not be read back.`
      : BigInt(collected.awaiting_cycle_msat) > 0n
        ? `${formatSats(collected.awaiting_cycle_msat)} stay locked until the next cycle turnover.`
        : '0 sats are waiting for the next cycle turnover.';

  if (collected.incomplete) {
    return `Claimed at least ${formatSats(collected.claimed_msat)} before the collection stopped at the ${collected.incomplete.phase.replace('_', ' ')} step. ${awaiting} Run it again.`;
  }
  return `Claimed ${formatSats(collected.claimed_msat)}. ${awaiting}`;
};
