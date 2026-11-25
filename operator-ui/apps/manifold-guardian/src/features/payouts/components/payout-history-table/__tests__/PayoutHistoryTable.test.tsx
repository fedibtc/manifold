import type { EcashPayoutState, PayoutJobStatusResponse } from '@operator-ui/types';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { PayoutHistoryTable } from '../PayoutHistoryTable';

const scope = {
  kind: 'payment_federation',
  federation_id: 'fed1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
} as const;
const operation = (amountMsat: number) => ({
  operation_id: 'op',
  amount_msat: amountMsat,
  capped: null,
  committed_at_ms: 1,
  confirmed_at_ms: null
});

const ecash = (
  requestId: string,
  state: EcashPayoutState | null,
  amountMsat = 40_000_000,
  confirmedAtMs: number | null = null
): PayoutJobStatusResponse => ({
  job: {
    request_id: requestId,
    scope,
    method: 'ecash',
    operation: { ...operation(amountMsat), confirmed_at_ms: confirmedAtMs },
    created_at_ms: 1_700_000_000_000
  },
  ecash: state
});

describe('PayoutHistoryTable', () => {
  it('should say what will be listed before the first withdrawal', () => {
    render(<PayoutHistoryTable payouts={[]} onOpen={vi.fn()} />);

    expect(screen.getByText(/No withdrawals yet/)).toBeInTheDocument();
  });

  it('should list each withdrawal with how it left and open its receipt', () => {
    const onOpen = vi.fn();
    render(
      <PayoutHistoryTable
        payouts={[
          ecash('returned', { state: 'reclaimed' }, 8_000_000),
          ecash('redeemed', { state: 'issued', token: null }, 25_000_000, 2)
        ]}
        onOpen={onOpen}
      />
    );

    const [, returned, redeemed] = screen.getAllByRole('row') as HTMLElement[];
    expect(returned).toHaveTextContent('8,000 sats');
    expect(within(returned as HTMLElement).getByText('Returned to balance')).toBeInTheDocument();
    expect(within(redeemed as HTMLElement).getByText('Redeemed')).toBeInTheDocument();
    fireEvent.click(
      within(redeemed as HTMLElement).getByRole('button', {
        name: /^Open the withdrawal of 25,000 sats/
      })
    );
    expect(onOpen).toHaveBeenCalledWith('redeemed');
  });
});
