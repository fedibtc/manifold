import type { PaymentFederation } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
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

const renderTable = (rows: PaymentFederation[]) => {
  const onWithdraw = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <PaymentSweepTable federations={rows} onWithdraw={onWithdraw} />
    </QueryClientProvider>
  );
  return onWithdraw;
};

describe('PaymentSweepTable', () => {
  it('should offer one Withdraw per payment federation, opening that federation', () => {
    const second = 'fed1bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
    const onWithdraw = renderTable([...federations, { ...federations[0], federation_id: second }]);

    const buttons = screen.getAllByRole('button', { name: /^Withdraw/ });
    expect(buttons).toHaveLength(2);
    fireEvent.click(buttons[1] as HTMLElement);
    expect(onWithdraw).toHaveBeenCalledWith(second);
  });

  it('should say there is nothing to withdraw from an empty wallet, with no button', () => {
    renderTable([{ ...federations[0], wallet: walletStatus(0) }]);

    expect(screen.getByText('Nothing to withdraw')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^Withdraw/ })).toBeNull();
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
    // The daemon decides whether an unread balance holds anything.
    expect(screen.getByRole('button', { name: /^Withdraw/ })).toBeEnabled();
  });

  // Carried over from the Wallet screen this table replaced. A stranded balance
  // raises one question — why is it stranded — and these are the two answers.
  it('should mark a federation that has stopped accepting payments', () => {
    renderTable([{ ...federations[0], receivable: false }]);

    expect(screen.getByText('Not accepting payments')).toBeInTheDocument();
  });

  it('should mark a federation the fleet no longer belongs to', () => {
    renderTable([{ ...federations[0], accepted: false }]);

    expect(screen.getByText('Former member')).toBeInTheDocument();
  });

  it('should mark a federation that is accepting payments', () => {
    renderTable(federations);

    expect(screen.getByText('Accepting payments')).toBeInTheDocument();
  });
});
