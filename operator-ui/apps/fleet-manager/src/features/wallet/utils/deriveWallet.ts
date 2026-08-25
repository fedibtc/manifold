import type { PaymentFederation } from '@operator-ui/types';
import { readTotalBalanceMsat } from '@/shared/utils/federationBalance';

export interface WalletModel {
  /** Null when at least one federation did not report a balance: a fleet total
   *  that silently drops an unread federation is not a total. */
  totalBalanceMsat: number | null;
  isEmpty: boolean;
}

export const deriveWallet = (federations: PaymentFederation[]): WalletModel => ({
  totalBalanceMsat: readTotalBalanceMsat(federations),
  isEmpty: federations.length === 0
});
