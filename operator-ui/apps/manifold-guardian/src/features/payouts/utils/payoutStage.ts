import type { ChipTone } from '@operator-ui/common-ui';
import type { PayoutJobStatusResponse } from '@operator-ui/types';

/** Where one payout stands for the operator: still moving (the In progress
 *  card), or finished (History), with the words its chip uses. */
export interface PayoutStage {
  open: boolean;
  label: string;
  tone: ChipTone;
}

/** A job whose start never committed a wallet operation. The daemon never
 *  lists one; only `PayoutStatus` of that request id answers it. */
const NOT_STARTED: PayoutStage = { open: false, label: 'Did not start', tone: 'neutral' };

/**
 * Read off the daemon's answer, never off what the dashboard asked for: a
 * Lightning payment is "Sent" only once its operation succeeded. An issued
 * ecash payout is open until the operator marks it redeemed; a later cancel's
 * outcome replaces the mark, because the wallet state comes first.
 */
export const payoutStage = (payout: PayoutJobStatusResponse): PayoutStage => {
  if ('payout' in payout) {
    switch (payout.payout?.state) {
      case undefined:
        return NOT_STARTED;
      case 'pending':
        return { open: true, label: 'Paying', tone: 'info' };
      case 'unknown':
        return { open: true, label: "Can't check right now", tone: 'neutral' };
      case 'succeeded':
        return { open: false, label: 'Sent', tone: 'neutral' };
      case 'failed_or_refunded':
        return { open: false, label: 'Failed, refunded', tone: 'warn' };
    }
  }
  switch (payout.ecash?.state) {
    case undefined:
      return NOT_STARTED;
    case 'issued':
      return payout.job.operation?.confirmed_at_ms
        ? { open: false, label: 'Redeemed', tone: 'neutral' }
        : { open: true, label: 'Waiting to be redeemed', tone: 'warn' };
    case 'reclaiming':
      return { open: true, label: 'Returning to balance', tone: 'info' };
    case 'already_spent':
      return { open: false, label: 'Redeemed', tone: 'neutral' };
    case 'reclaimed':
      return { open: false, label: 'Returned to balance', tone: 'neutral' };
  }
};

/** A payout moving on its own: a payment or a reclaim the federation has not
 *  settled. An issued token waits for the operator instead, so the dashboard
 *  polls fast only for these. */
export const movesOnItsOwn = (payout: PayoutJobStatusResponse): boolean =>
  payoutStage(payout).open && !('ecash' in payout && payout.ecash?.state === 'issued');

/** How the payout left, for History. */
export const payoutMethodLabel = (payout: PayoutJobStatusResponse): string =>
  payout.job.method === 'lightning' ? 'Lightning' : 'Ecash';
