import type { PaymentFederation } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { walletStatus } from '@/mocks/wallet-status';
import { PaymentSweepTable } from '../PaymentSweepTable';

const federations: PaymentFederation[] = [
  {
    federation_id: 'fed1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
    accepted: true,
    receivable: true,
    wallet: walletStatus(250_000_000)
  }
];

const renderTable = (rows: PaymentFederation[], hasDestination = true) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <PaymentSweepTable federations={rows} hasDestination={hasDestination} />
    </QueryClientProvider>
  );
};

describe('PaymentSweepTable', () => {
  it('should offer one sweep per payment federation', () => {
    renderTable([
      ...federations,
      { ...federations[0], federation_id: 'fed1bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb' }
    ]);

    expect(screen.getAllByRole('button', { name: 'Sweep' })).toHaveLength(2);
  });

  it('should show the balance a sweep would move', () => {
    renderTable(federations);

    expect(screen.getByText('250,000 sats')).toBeInTheDocument();
  });

  // An unread balance is a different fact from an empty wallet, and the repo
  // renders it as a dash rather than as zero.
  it('should render an unread balance as unknown rather than as zero', () => {
    renderTable([{ ...federations[0], wallet: walletStatus(null) }]);

    expect(screen.getByText('—')).toBeInTheDocument();
  });

  it('should gate every row when no payout destination is stored', () => {
    renderTable(federations, false);

    expect(screen.getByRole('button', { name: 'Sweep' })).toBeDisabled();
  });
});
