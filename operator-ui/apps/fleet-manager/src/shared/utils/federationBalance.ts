import type { PaymentFederation } from '@operator-ui/types';

/**
 * The fleet's balance across its payment federations, or null when at least one
 * federation did not report one.
 *
 * The daemon reports unreadable available ecash as null in the wallet projection,
 * so folding that into the sum as a zero would state "this wallet holds nothing"
 * where the truth is "this wallet could not be read". A total that
 * silently drops an unread wallet is not a total: it goes unknown instead, and
 * the money screens render that as "—".
 *
 * Every screen showing a fleet-wide balance reads it from here, so the Wallet and
 * the Overview cannot disagree about the same money.
 */
export const readTotalBalanceMsat = (federations: PaymentFederation[]): number | null =>
  federations.some((federation) => federation.wallet.available_ecash_msat === null)
    ? null
    : federations.reduce(
        (total, federation) => total + (federation.wallet.available_ecash_msat ?? 0),
        0
      );
