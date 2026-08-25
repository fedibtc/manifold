import type { PaymentFederation } from '@operator-ui/types';
import { describe, expect, it } from 'vitest';
import { walletStatus } from '@/mocks/wallet-status';
import { deriveWallet } from '../deriveWallet';

const federation = (id: string, balanceMsat: number | null): PaymentFederation => ({
  federation_id: id,
  accepted: true,
  receivable: true,
  wallet: walletStatus(balanceMsat)
});

describe('deriveWallet', () => {
  it('should report empty with a zero total for no federations', () => {
    const model = deriveWallet([]);

    expect(model.isEmpty).toBe(true);
    expect(model.totalBalanceMsat).toBe(0);
  });

  it('should sum federation balances when every federation reported one', () => {
    const model = deriveWallet([federation('fed-a', 5_000_000), federation('fed-b', 2_500_000)]);

    expect(model.isEmpty).toBe(false);
    expect(model.totalBalanceMsat).toBe(7_500_000);
  });

  it('should report an unknown total when a federation did not report its balance', () => {
    const model = deriveWallet([federation('fed-a', 5_000_000), federation('fed-b', null)]);

    expect(model.isEmpty).toBe(false);
    expect(model.totalBalanceMsat).toBeNull();
  });
});
