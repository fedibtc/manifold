import type { CreateDepositAddressResponse, RequestWithdrawalResponse } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('@/shared/api/adminCall', () => ({ adminCall: vi.fn() }));

import { FUNDS_KEY, WALLET_OPERATIONS_KEY } from '@/features/funds/api/hooks/use-funds/useFunds';
import { FundsActions } from '@/features/funds/components/funds-actions/FundsActions';
import { adminCall } from '@/shared/api/adminCall';
import { AdminApiError } from '@/shared/api/errors';

const depositResponse: CreateDepositAddressResponse = {
  address: 'bcrt1qmockdepositaddress0001',
  network: 'regtest'
};

const withdrawalResponse: RequestWithdrawalResponse = {
  operation: {
    operation_id: 'wop_withdraw_1',
    operation_type: 'withdrawal',
    amount: 100_000,
    status: 'pending',
    created_at: 1721304000,
    updated_at: 1721304000
  }
};

const mockedAdminCall = vi.mocked(adminCall);

const renderActions = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const invalidateSpy = vi.spyOn(client, 'invalidateQueries');
  render(
    <QueryClientProvider client={client}>
      <FundsActions />
    </QueryClientProvider>
  );
  return invalidateSpy;
};

describe('FundsActions', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    mockedAdminCall.mockReset();
  });

  it('should render the top-up and withdraw actions', () => {
    renderActions();

    expect(screen.getByRole('button', { name: 'Top up' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Withdraw' })).toBeTruthy();
  });

  it('should reveal the top-up panel with the deposit address when topping up', async () => {
    mockedAdminCall.mockImplementation((method: string) =>
      method === 'create_deposit_address'
        ? Promise.resolve(depositResponse)
        : Promise.resolve({ operations: { items: [], next_page: null } })
    );

    renderActions();
    fireEvent.click(screen.getByRole('button', { name: 'Top up' }));

    expect(await screen.findByText(depositResponse.address)).toBeTruthy();
  });

  it('should reveal the withdrawal form when withdraw is chosen', () => {
    renderActions();
    fireEvent.click(screen.getByRole('button', { name: 'Withdraw' }));

    expect(screen.getByText('Withdrawal address')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Request withdrawal' })).toBeTruthy();
  });

  it.each([
    { name: 'empty address', address: '', amount: '1000' },
    { name: 'zero amount', address: 'bcrt1qmockwithdrawaddress0001', amount: '0' },
    { name: 'negative amount', address: 'bcrt1qmockwithdrawaddress0001', amount: '-1' },
    { name: 'non-numeric amount', address: 'bcrt1qmockwithdrawaddress0001', amount: 'abc' },
    { name: 'fractional amount', address: 'bcrt1qmockwithdrawaddress0001', amount: '1.5' }
  ])('should disable withdrawal submit for $name', ({ address, amount }) => {
    renderActions();
    fireEvent.click(screen.getByRole('button', { name: 'Withdraw' }));

    fireEvent.change(screen.getByLabelText('Withdrawal address'), { target: { value: address } });
    fireEvent.change(screen.getByLabelText('Amount (sats)'), { target: { value: amount } });

    const submitButton = screen.getByRole('button', { name: 'Request withdrawal' });
    expect(submitButton.hasAttribute('disabled')).toBe(true);

    fireEvent.click(submitButton);
    expect(mockedAdminCall).not.toHaveBeenCalled();
  });

  it('should invalidate funds and wallet-operations after a withdrawal succeeds', async () => {
    mockedAdminCall.mockResolvedValue(withdrawalResponse);
    const invalidateSpy = renderActions();

    fireEvent.click(screen.getByRole('button', { name: 'Withdraw' }));
    fireEvent.change(screen.getByLabelText('Withdrawal address'), {
      target: { value: 'bcrt1qmockwithdrawaddress0001' }
    });
    fireEvent.change(screen.getByLabelText('Amount (sats)'), { target: { value: '1000' } });
    fireEvent.click(screen.getByRole('button', { name: 'Request withdrawal' }));

    expect(await screen.findByText(/wop_withdraw_1/)).toBeTruthy();
    expect(mockedAdminCall).toHaveBeenCalledWith(
      'request_withdrawal',
      expect.objectContaining({ address: 'bcrt1qmockwithdrawaddress0001', amount: 1000 })
    );
    await waitFor(() => {
      expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: FUNDS_KEY });
      expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: WALLET_OPERATIONS_KEY });
    });
  });

  it('should surface an inline error when a withdrawal request fails', async () => {
    mockedAdminCall.mockRejectedValue(new AdminApiError('invalid_argument', 'amount too high'));
    renderActions();

    fireEvent.click(screen.getByRole('button', { name: 'Withdraw' }));
    fireEvent.change(screen.getByLabelText('Withdrawal address'), {
      target: { value: 'bcrt1qmockwithdrawaddress0001' }
    });
    fireEvent.change(screen.getByLabelText('Amount (sats)'), { target: { value: '1000' } });
    fireEvent.click(screen.getByRole('button', { name: 'Request withdrawal' }));

    expect(await screen.findByText(/invalid_argument: amount too high/)).toBeTruthy();
  });
});
