import type { PaymentFederation } from '@operator-ui/types';
import { describe, expect, it } from 'vitest';
import { walletStatus } from '@/mocks/wallet-status';
import { readTotalBalanceMsat } from '../federationBalance';

const federation = (id: string, balanceMsat: number | null): PaymentFederation => ({
  federation_id: id,
  accepted: true,
  receivable: true,
  wallet: walletStatus(balanceMsat)
});

describe('readTotalBalanceMsat', () => {
  it('should total nothing as zero when there are no federations', () => {
    expect(readTotalBalanceMsat([])).toBe(0);
  });

  it('should sum the balances when every federation reported one', () => {
    expect(
      readTotalBalanceMsat([federation('fed-a', 5_000_000), federation('fed-b', 2_500_000)])
    ).toBe(7_500_000);
  });

  it('should report an unknown total when a single federation did not report a balance', () => {
    expect(readTotalBalanceMsat([federation('fed-a', null)])).toBeNull();
  });

  it('should report an unknown total when one federation of several is unreadable', () => {
    expect(
      readTotalBalanceMsat([
        federation('fed-a', 5_000_000),
        federation('fed-b', null),
        federation('fed-c', 2_500_000)
      ])
    ).toBeNull();
  });
});
