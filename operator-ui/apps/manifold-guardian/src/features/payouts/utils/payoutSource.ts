import { truncateMiddle } from '@operator-ui/common-ui';
import type { PayoutScope } from '@operator-ui/types';

/** The balance a withdrawal empties: one payment federation's seat sales, or
 *  one seat's guardian fees. */
export type PayoutSource =
  | { kind: 'payment_federation'; federationId: string }
  | { kind: 'guardian_fee'; seatId: string };

/** The balance a payout job emptied. */
export const sourceOfScope = (scope: PayoutScope): PayoutSource =>
  scope.kind === 'payment_federation'
    ? { kind: 'payment_federation', federationId: scope.federation_id }
    : { kind: 'guardian_fee', seatId: scope.seat_id };

/** The dialog title: what is being withdrawn. */
export const withdrawTitle = (source: PayoutSource): string =>
  source.kind === 'payment_federation' ? 'Withdraw seat sales' : 'Withdraw guardian fees';

/** The identity under the title, copyable in full. */
export const sourceId = (source: PayoutSource): string =>
  source.kind === 'payment_federation' ? source.federationId : source.seatId;

/** Which balance, in the words of the two revenue tables. */
export const describeSource = (source: PayoutSource): string =>
  source.kind === 'payment_federation'
    ? `Seat sales · ${truncateMiddle(source.federationId, 8, 8)}`
    : `Guardian fees · ${truncateMiddle(source.seatId, 8, 8)}`;
