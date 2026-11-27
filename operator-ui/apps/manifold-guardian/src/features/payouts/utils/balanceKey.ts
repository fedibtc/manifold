import type { PayoutSource } from '@/features/payouts/utils/payoutSource';
import { guardianFeesKey } from '@/shared/api/hooks/use-guardian-fees/useGuardianFees';
import { PAYMENT_FEDERATIONS_KEY } from '@/shared/api/hooks/use-payment-federations/usePaymentFederations';

/** The query holding the balance a payout from `source` empties or refills. */
export const balanceKeyFor = (source: PayoutSource) =>
  source.kind === 'payment_federation' ? PAYMENT_FEDERATIONS_KEY : guardianFeesKey(source.seatId);
