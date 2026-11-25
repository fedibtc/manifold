import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { PayoutAddressDialog } from '../PayoutAddressDialog';

const storeAnswer = () =>
  vi
    .spyOn(adminCallModule, 'adminCall')
    .mockImplementation(((request: { SetPayoutDestination: { destination: string | null } }) =>
      Promise.resolve(request.SetPayoutDestination)) as typeof adminCallModule.adminCall);

const renderDialog = (destination: string | null) => {
  const onDone = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <PayoutAddressDialog destination={destination} onClose={vi.fn()} onDone={onDone} />
    </QueryClientProvider>
  );
  return onDone;
};

const field = () => screen.getByLabelText('Lightning address or LNURL-pay');

afterEach(() => {
  vi.restoreAllMocks();
});

describe('PayoutAddressDialog', () => {
  it('should refuse what is not a Lightning address or LNURL without storing it', () => {
    const adminCall = storeAnswer();
    renderDialog(null);

    fireEvent.change(field(), { target: { value: 'lnbc1invoice' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save address' }));

    expect(screen.getByText(/Enter a Lightning address/)).toBeInTheDocument();
    expect(adminCall).not.toHaveBeenCalled();
  });

  it('should store a first address without a confirmation', async () => {
    const adminCall = storeAnswer();
    const onDone = renderDialog(null);

    fireEvent.change(field(), { target: { value: 'operator@example.com' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save address' }));

    await waitFor(() => expect(onDone).toHaveBeenCalledOnce());
    expect(adminCall).toHaveBeenCalledWith({
      SetPayoutDestination: { destination: 'operator@example.com' }
    });
  });

  // Every later Lightning withdrawal goes where the address points, so a
  // change is shown from -> to and stored only once confirmed.
  it('should confirm a change from the old address to the new one before storing it', async () => {
    const adminCall = storeAnswer();
    const onDone = renderDialog('old@example.com');

    fireEvent.change(field(), { target: { value: 'new@example.com' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save address' }));

    expect(screen.getByRole('dialog', { name: 'Change payout address?' })).toHaveTextContent(
      'Fromold@example.comTonew@example.com'
    );
    expect(adminCall).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Change address' }));
    await waitFor(() => expect(onDone).toHaveBeenCalledOnce());
    expect(adminCall).toHaveBeenCalledWith({
      SetPayoutDestination: { destination: 'new@example.com' }
    });
  });

  it('should remove the address only after its confirmation', async () => {
    const adminCall = storeAnswer();
    const onDone = renderDialog('old@example.com');

    fireEvent.click(screen.getByRole('button', { name: 'Remove address' }));
    expect(adminCall).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Remove address' }));

    await waitFor(() => expect(onDone).toHaveBeenCalledOnce());
    expect(adminCall).toHaveBeenCalledWith({ SetPayoutDestination: { destination: null } });
  });
});
