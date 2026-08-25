import type { PaymentFederation } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { walletStatus } from '@/mocks/wallet-status';
import { FederationTable } from '../FederationTable';

const federations: PaymentFederation[] = [
  {
    federation_id: 'fed1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
    accepted: true,
    receivable: false,
    wallet: walletStatus(5_000_000)
  }
];

const renderTable = (rows: PaymentFederation[]) => render(<FederationTable federations={rows} />);

describe('FederationTable', () => {
  it('should render the not-receiving status and sats balance for a federation', () => {
    renderTable(federations);

    expect(screen.getByText('Not receiving')).toBeTruthy();
    expect(screen.getByText('5,000 sats')).toBeTruthy();
  });

  it('should render the receivable status when the federation can receive', () => {
    renderTable([{ ...federations[0], receivable: true }]);

    expect(screen.getByText('Receivable')).toBeTruthy();
  });

  it('should mark a federation that is no longer in the accepted set as a former member', () => {
    renderTable([{ ...federations[0], accepted: false }]);

    expect(screen.getByText('Former member')).toBeTruthy();
  });

  // Moving money out is on Payouts, not here: a sweep needs the payout
  // destination this table knows nothing about, so the row has nothing to act on.
  it('should offer no row action, because the row has nothing to act on', () => {
    renderTable(federations);

    expect(screen.queryByRole('link')).toBeNull();
  });
});
