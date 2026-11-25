import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mockEcashToken } from '@/mocks/ecash-token';
import { getState, resetState } from '@/mocks/state';
import { dispatch } from '@/mocks/world/verbs';
import * as adminCallModule from '@/shared/api/adminCall';
import { AdminApiError } from '@/shared/api/errors';
import { PayoutsPage } from '../PayoutsPage';

type Scenario = Parameters<typeof resetState>[0];

// The mock world stands in for the daemon, so a flow runs against the same
// verbs, idempotency and token handling the dashboard meets in the browser.
const renderPage = (scenario: Scenario, adjust: () => void = () => {}) => {
  resetState(scenario);
  adjust();
  const adminCall = vi.spyOn(adminCallModule, 'adminCall').mockImplementation(((
    request: unknown
  ) => {
    const result = dispatch(request) as { Ok: unknown } | { Err: { message: string } };
    // A copy, as the wire gives: the mock answers with its live objects.
    return 'Ok' in result
      ? Promise.resolve(structuredClone(result.Ok))
      : Promise.reject(new AdminApiError(result.Err.message));
  }) as typeof adminCallModule.adminCall);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <PayoutsPage />
    </QueryClientProvider>
  );
  return adminCall;
};

const seatSalesRow = async (federationPrefix: string) => {
  const table = (await screen.findByRole('heading', { name: 'Seat sales' })).closest(
    'section'
  ) as HTMLElement;
  const cell = await within(table).findByText(new RegExp(`^${federationPrefix}`));
  return cell.closest('tr') as HTMLElement;
};

const openSeatSales = async () => {
  fireEvent.click(
    within(await seatSalesRow('fed1aaaa')).getByRole('button', { name: /^Withdraw/ })
  );
  return screen.findByRole('dialog', { name: 'Withdraw seat sales' });
};

afterEach(() => {
  vi.restoreAllMocks();
  resetState('not-onboarded');
});

describe('PayoutsPage', () => {
  it('should claim nothing before the daemon has answered', () => {
    resetState('earnings');
    vi.spyOn(adminCallModule, 'adminCall').mockReturnValue(new Promise(() => {}));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <PayoutsPage />
      </QueryClientProvider>
    );

    expect(screen.getAllByText('Loading…').length).toBeGreaterThan(0);
    expect(screen.queryAllByRole('button', { name: /^Withdraw/ })).toHaveLength(0);
  });

  it('should review a Lightning withdrawal with its fee and what arrives', async () => {
    renderPage('earnings');
    const dialog = await openSeatSales();

    // 150,000 sats leave; the mock's gateway keeps 0.5% back.
    expect(within(dialog).getByRole('radio', { name: 'Lightning' })).toBeChecked();
    expect(within(dialog).getByText('≈ 750 sats')).toBeInTheDocument();
    expect(within(dialog).getByText('You receive about 149,250 sats')).toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: 'Withdraw 149,250 sats' })).toBeEnabled();
  });

  // The receipt comes from the payout's status, which the mock settles at once.
  it('should withdraw over Lightning to a receipt', async () => {
    renderPage('earnings');
    const dialog = await openSeatSales();

    fireEvent.click(within(dialog).getByRole('button', { name: 'Withdraw 149,250 sats' }));

    expect(await screen.findByText('Sent 149,250 sats')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Copy operation ID' })).toBeInTheDocument();
  });

  it('should withdraw as ecash, show the code, and take it back after a mark', async () => {
    const adminCall = renderPage('earnings');
    const dialog = await openSeatSales();

    fireEvent.click(within(dialog).getByRole('radio', { name: 'Ecash' }));
    expect(within(dialog).getByText('One ecash code for 150,000 sats')).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Withdraw 150,000 sats as ecash' }));

    expect(await screen.findByText('150,000 sats of ecash')).toBeInTheDocument();
    expect(screen.getByRole('img', { name: 'Ecash QR code, animated' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Copy ecash' })).toBeInTheDocument();
    const requestId = getState().payouts[0]?.job.request_id as string;
    // The page's own list carries no token.
    await waitFor(() => expect(adminCall).toHaveBeenCalledWith('ListPayouts'));
    const listCalls = adminCall.mock.results.filter(
      (_, index) => adminCall.mock.calls[index]?.[0] === 'ListPayouts'
    );
    for (const call of listCalls) {
      expect(JSON.stringify(await call.value)).not.toContain(mockEcashToken(requestId));
    }

    fireEvent.click(screen.getByRole('button', { name: "I've redeemed it" }));
    expect(await screen.findByText('Redeemed 150,000 sats')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: "Money didn't arrive? Take it back" }));
    expect(screen.getByText('Cancel this withdrawal?')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel withdrawal' }));

    expect(await screen.findByText('Returned 150,000 sats')).toBeInTheDocument();
    expect(adminCall).toHaveBeenCalledWith({ CancelEcashPayout: { request_id: requestId } });
  });

  it('should keep Cancel withdrawal behind a confirmation that swaps the code out', async () => {
    renderPage('earnings');
    const dialog = await openSeatSales();
    fireEvent.click(within(dialog).getByRole('radio', { name: 'Ecash' }));
    fireEvent.click(within(dialog).getByRole('button', { name: 'Withdraw 150,000 sats as ecash' }));
    await screen.findByText('150,000 sats of ecash');

    fireEvent.click(screen.getByRole('button', { name: 'Cancel withdrawal' }));

    expect(screen.getAllByRole('dialog')).toHaveLength(1);
    expect(screen.queryByRole('img', { name: /Ecash QR code/ })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Keep ecash' }));
    expect(screen.getByRole('img', { name: /Ecash QR code/ })).toBeInTheDocument();
  });

  it('should default to ecash and block Lightning while no payout address is saved', async () => {
    renderPage('earnings', () => {
      getState().payoutDestination = null;
    });
    const dialog = await openSeatSales();

    expect(within(dialog).getByRole('radio', { name: 'Ecash' })).toBeChecked();
    fireEvent.click(within(dialog).getByRole('radio', { name: 'Lightning' }));
    expect(within(dialog).getByText('No payout address yet.')).toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: 'Withdraw 149,250 sats' })).toBeDisabled();
  });

  it('should list a waiting token without the token, and History with each outcome', async () => {
    renderPage('ecash-payout-open');

    const inProgress = (await screen.findByRole('heading', { name: 'In progress' })).closest(
      'section'
    ) as HTMLElement;
    expect(within(inProgress).getByText('Waiting to be redeemed')).toBeInTheDocument();
    expect(document.body.textContent).not.toContain(mockEcashToken('mock-ecash-payout-open'));

    const history = (await screen.findByRole('heading', { name: 'History' })).closest(
      'section'
    ) as HTMLElement;
    for (const label of ['Sent', 'Redeemed', 'Returned to balance', 'Failed, refunded']) {
      expect(await within(history).findByText(label)).toBeInTheDocument();
    }

    fireEvent.click(within(inProgress).getByRole('button', { name: 'Show ecash' }));
    expect(await screen.findByText('40,000 sats of ecash')).toBeInTheDocument();
  });

  it('should collect the fee pool inside the guardian-fees dialog', async () => {
    renderPage('earnings');
    const fees = (await screen.findByRole('heading', { name: 'Guardian fees' })).closest(
      'section'
    ) as HTMLElement;
    const row = (await within(fees).findByText('seat-earning-01')).closest('tr') as HTMLElement;
    await waitFor(() =>
      expect(within(row).getByRole('button', { name: /^Withdraw/ })).toBeEnabled()
    );

    fireEvent.click(within(row).getByRole('button', { name: /^Withdraw/ }));
    const dialog = await screen.findByRole('dialog', { name: 'Withdraw guardian fees' });
    const collect = await within(dialog).findByRole('button', { name: /^Collect/ });
    expect(within(dialog).getByText(/are still in the fee pool/)).toBeInTheDocument();
    fireEvent.click(collect);

    expect(await within(dialog).findByText(/^Claimed/)).toBeInTheDocument();
  });

  it('should confirm a payout address change from the old address to the new one', async () => {
    renderPage('earnings');

    fireEvent.click(await screen.findByRole('button', { name: 'Change' }));
    const dialog = await screen.findByRole('dialog', { name: 'Change payout address' });
    fireEvent.change(within(dialog).getByLabelText('Lightning address or LNURL-pay'), {
      target: { value: 'new@example.com' }
    });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Save address' }));

    const confirm = await screen.findByRole('dialog', { name: 'Change payout address?' });
    expect(within(confirm).getByText('operator@example.com')).toBeInTheDocument();
    expect(within(confirm).getByText('new@example.com')).toBeInTheDocument();
    expect(getState().payoutDestination).toBe('operator@example.com');
    fireEvent.click(within(confirm).getByRole('button', { name: 'Change address' }));

    await waitFor(() => expect(getState().payoutDestination).toBe('new@example.com'));
  });
});
