import type { EcashPayoutState, PayoutJobStatusResponse } from '@operator-ui/types';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { InProgressCard } from '../InProgressCard';

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
  amountMsat = 40_000_000
): PayoutJobStatusResponse => ({
  job: {
    request_id: requestId,
    scope,
    method: 'ecash',
    operation: operation(amountMsat),
    created_at_ms: 1_700_000_000_000
  },
  ecash: state
});

const lightning = (requestId: string, amountMsat = 150_000_000): PayoutJobStatusResponse => ({
  job: {
    request_id: requestId,
    scope,
    method: 'lightning',
    destination: 'operator@example.com',
    operation: operation(amountMsat),
    created_at_ms: 1_700_000_000_000
  },
  payout: {
    operation_id: 'op',
    rail: 'lnv2',
    state: 'pending',
    recipient_amount_msat: amountMsat,
    contract_amount_msat: amountMsat,
    encumbered_msat: amountMsat,
    has_active_state_machines: true
  }
});

describe('InProgressCard', () => {
  // Only an issued token has a code to show; a reclaim in flight has none.
  it('should offer Show ecash only on an issued token', () => {
    const onOpen = vi.fn();
    render(
      <InProgressCard
        payouts={[
          lightning('ln'),
          ecash('returning', { state: 'reclaiming' }),
          ecash('issued', { state: 'issued', token: null })
        ]}
        onOpen={onOpen}
      />
    );

    const [paying, returning, issued] = screen.getAllByRole('listitem') as [
      HTMLElement,
      HTMLElement,
      HTMLElement
    ];
    expect(paying).toHaveTextContent('Lightning to operator@example.com');
    expect(within(paying).queryByRole('button')).toBeNull();
    expect(returning).toHaveTextContent('Returning to balance');
    expect(within(returning).queryByRole('button')).toBeNull();
    fireEvent.click(within(issued).getByRole('button', { name: 'Show ecash' }));
    expect(onOpen).toHaveBeenCalledWith('issued');
  });
});
