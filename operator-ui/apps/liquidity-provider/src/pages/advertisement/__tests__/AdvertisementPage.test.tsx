import type { GetAdvertisementStateResponse } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as advertisementStateHooks from '@/features/advertisement/hooks/use-advertisement-state/useAdvertisementState';
import * as refreshRelaysHooks from '@/features/advertisement/hooks/use-refresh-relays/useRefreshRelays';
import * as republishHooks from '@/features/advertisement/hooks/use-republish-advertisement/useRepublishAdvertisement';
import * as withdrawHooks from '@/features/advertisement/hooks/use-withdraw-advertisement/useWithdrawAdvertisement';
import { AdvertisementPage } from '../AdvertisementPage';

const renderPage = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <AdvertisementPage />
    </QueryClientProvider>
  );
};

type AdStateResult = ReturnType<typeof advertisementStateHooks.useAdvertisementState>;
type RepublishResult = ReturnType<typeof republishHooks.useRepublishAdvertisement>;
type WithdrawResult = ReturnType<typeof withdrawHooks.useWithdrawAdvertisement>;
type RefreshResult = ReturnType<typeof refreshRelaysHooks.useRefreshRelays>;

const asAdStateResult = (partial: Partial<AdStateResult>): AdStateResult =>
  partial as unknown as AdStateResult;
const asRepublishResult = (partial: Partial<RepublishResult>): RepublishResult =>
  partial as unknown as RepublishResult;
const asWithdrawResult = (partial: Partial<WithdrawResult>): WithdrawResult =>
  partial as unknown as WithdrawResult;
const asRefreshResult = (partial: Partial<RefreshResult>): RefreshResult =>
  partial as unknown as RefreshResult;

const publishedData: GetAdvertisementStateResponse = {
  advertisement: {
    payload: {
      version: 1,
      provider_pubkey: 'npub1qy3q6x8vhmz6q6x8vhmz6q6x8vhmzkzsx',
      issued_at: 1721476800,
      expires_at: 1721563200,
      supported_sources: ['gateway'],
      holder_authorizations: [],
      policy: {} as never,
      display: null,
      api_endpoints: ['iroh:b3f9d2c2ae91c2ae'],
      api_versions: [1],
      relay_hints: ['wss://relay.signet.example']
    },
    proof: { signature: [] }
  },
  publication_status: 'published',
  last_published_at: 1721476800,
  expires_at: 1721563200,
  withdrawn_at: null,
  relay_states: [],
  ready: true,
  readiness: { status: 'passed', checks: [] },
  unverified_holder_authorization_count: 0
};

const notReadyData: GetAdvertisementStateResponse = {
  advertisement: null,
  publication_status: 'not_ready',
  last_published_at: null,
  expires_at: null,
  withdrawn_at: null,
  relay_states: [],
  ready: false,
  readiness: {
    status: 'failed',
    checks: [
      {
        name: 'gateway_reachability',
        status: 'failed',
        detail: 'gateway admin_url did not respond'
      },
      { name: 'relays_reachable', status: 'failed', detail: 'no relays configured' },
      { name: 'other_check', status: 'passed', detail: null }
    ]
  },
  unverified_holder_authorization_count: 0
};

const mockRefresh = () => {
  vi.spyOn(refreshRelaysHooks, 'useRefreshRelays').mockReturnValue(
    asRefreshResult({ mutate: vi.fn(), isPending: false })
  );
};

describe('AdvertisementPage', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('should not call withdraw.mutate when Withdraw advertisement is clicked, but reveal the confirm panel', () => {
    const withdrawMutate = vi.fn();
    vi.spyOn(advertisementStateHooks, 'useAdvertisementState').mockReturnValue(
      asAdStateResult({ isLoading: false, isError: false, data: publishedData })
    );
    vi.spyOn(republishHooks, 'useRepublishAdvertisement').mockReturnValue(
      asRepublishResult({ mutate: vi.fn(), isPending: false })
    );
    vi.spyOn(withdrawHooks, 'useWithdrawAdvertisement').mockReturnValue(
      asWithdrawResult({ mutate: withdrawMutate, isPending: false })
    );
    mockRefresh();

    renderPage();
    fireEvent.click(screen.getByRole('button', { name: 'Withdraw advertisement' }));

    expect(withdrawMutate).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Confirm withdrawal' })).toBeTruthy();
  });

  it('should call withdraw.mutate with the reason when Confirm withdrawal is clicked', () => {
    const withdrawMutate = vi.fn();
    vi.spyOn(advertisementStateHooks, 'useAdvertisementState').mockReturnValue(
      asAdStateResult({ isLoading: false, isError: false, data: publishedData })
    );
    vi.spyOn(republishHooks, 'useRepublishAdvertisement').mockReturnValue(
      asRepublishResult({ mutate: vi.fn(), isPending: false })
    );
    vi.spyOn(withdrawHooks, 'useWithdrawAdvertisement').mockReturnValue(
      asWithdrawResult({ mutate: withdrawMutate, isPending: false })
    );
    mockRefresh();

    renderPage();
    fireEvent.click(screen.getByRole('button', { name: 'Withdraw advertisement' }));
    fireEvent.change(screen.getByLabelText('Reason (optional)'), {
      target: { value: 'maintenance' }
    });
    fireEvent.click(screen.getByRole('button', { name: 'Confirm withdrawal' }));

    expect(withdrawMutate).toHaveBeenCalledWith('maintenance', expect.anything());
  });

  it('should hide the confirm panel without mutating when Cancel is clicked', () => {
    const withdrawMutate = vi.fn();
    vi.spyOn(advertisementStateHooks, 'useAdvertisementState').mockReturnValue(
      asAdStateResult({ isLoading: false, isError: false, data: publishedData })
    );
    vi.spyOn(republishHooks, 'useRepublishAdvertisement').mockReturnValue(
      asRepublishResult({ mutate: vi.fn(), isPending: false })
    );
    vi.spyOn(withdrawHooks, 'useWithdrawAdvertisement').mockReturnValue(
      asWithdrawResult({ mutate: withdrawMutate, isPending: false })
    );
    mockRefresh();

    renderPage();
    fireEvent.click(screen.getByRole('button', { name: 'Withdraw advertisement' }));
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));

    expect(withdrawMutate).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: 'Confirm withdrawal' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Withdraw advertisement' })).toBeTruthy();
  });

  it('should move focus into the confirm panel so the prompt is announced', () => {
    vi.spyOn(advertisementStateHooks, 'useAdvertisementState').mockReturnValue(
      asAdStateResult({ isLoading: false, isError: false, data: publishedData })
    );
    vi.spyOn(republishHooks, 'useRepublishAdvertisement').mockReturnValue(
      asRepublishResult({ mutate: vi.fn(), isPending: false })
    );
    vi.spyOn(withdrawHooks, 'useWithdrawAdvertisement').mockReturnValue(
      asWithdrawResult({ mutate: vi.fn(), isPending: false })
    );
    mockRefresh();

    renderPage();
    fireEvent.click(screen.getByRole('button', { name: 'Withdraw advertisement' }));

    expect(document.activeElement).toBe(
      screen.getByRole('group', { name: 'Withdraw this advertisement?' })
    );
  });

  it('should return focus to the withdraw trigger when the confirm panel closes', () => {
    vi.spyOn(advertisementStateHooks, 'useAdvertisementState').mockReturnValue(
      asAdStateResult({ isLoading: false, isError: false, data: publishedData })
    );
    vi.spyOn(republishHooks, 'useRepublishAdvertisement').mockReturnValue(
      asRepublishResult({ mutate: vi.fn(), isPending: false })
    );
    vi.spyOn(withdrawHooks, 'useWithdrawAdvertisement').mockReturnValue(
      asWithdrawResult({ mutate: vi.fn(), isPending: false })
    );
    mockRefresh();

    renderPage();
    fireEvent.click(screen.getByRole('button', { name: 'Withdraw advertisement' }));
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));

    expect(document.activeElement).toBe(
      screen.getByRole('button', { name: 'Withdraw advertisement' })
    );
  });

  it('should close the confirm panel on Escape without mutating', () => {
    const withdrawMutate = vi.fn();
    vi.spyOn(advertisementStateHooks, 'useAdvertisementState').mockReturnValue(
      asAdStateResult({ isLoading: false, isError: false, data: publishedData })
    );
    vi.spyOn(republishHooks, 'useRepublishAdvertisement').mockReturnValue(
      asRepublishResult({ mutate: vi.fn(), isPending: false })
    );
    vi.spyOn(withdrawHooks, 'useWithdrawAdvertisement').mockReturnValue(
      asWithdrawResult({ mutate: withdrawMutate, isPending: false })
    );
    mockRefresh();

    renderPage();
    fireEvent.click(screen.getByRole('button', { name: 'Withdraw advertisement' }));
    fireEvent.keyDown(screen.getByRole('group', { name: 'Withdraw this advertisement?' }), {
      key: 'Escape'
    });

    expect(withdrawMutate).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: 'Confirm withdrawal' })).toBeNull();
  });

  it('should keep the listing visible under a stale banner when a poll fails', () => {
    vi.spyOn(advertisementStateHooks, 'useAdvertisementState').mockReturnValue(
      asAdStateResult({
        isLoading: false,
        isError: true,
        data: publishedData,
        dataUpdatedAt: 1721476800000
      })
    );
    vi.spyOn(republishHooks, 'useRepublishAdvertisement').mockReturnValue(
      asRepublishResult({ mutate: vi.fn(), isPending: false })
    );
    vi.spyOn(withdrawHooks, 'useWithdrawAdvertisement').mockReturnValue(
      asWithdrawResult({ mutate: vi.fn(), isPending: false })
    );
    mockRefresh();

    renderPage();

    expect(screen.getByText('Published')).toBeTruthy();
    expect(screen.getByText('Showing last-known data')).toBeTruthy();
    expect(screen.queryByText("Couldn't load advertisement state")).toBeNull();
  });

  it('should show the error state only when there is no advertisement state at all', () => {
    vi.spyOn(advertisementStateHooks, 'useAdvertisementState').mockReturnValue(
      asAdStateResult({ isLoading: false, isError: true, data: undefined })
    );
    vi.spyOn(republishHooks, 'useRepublishAdvertisement').mockReturnValue(
      asRepublishResult({ mutate: vi.fn(), isPending: false })
    );
    vi.spyOn(withdrawHooks, 'useWithdrawAdvertisement').mockReturnValue(
      asWithdrawResult({ mutate: vi.fn(), isPending: false })
    );
    mockRefresh();

    renderPage();

    expect(screen.getByText("Couldn't load advertisement state")).toBeTruthy();
  });

  it('should disable Republish and surface blocking reasons when publication_status is not_ready', () => {
    vi.spyOn(advertisementStateHooks, 'useAdvertisementState').mockReturnValue(
      asAdStateResult({ isLoading: false, isError: false, data: notReadyData })
    );
    vi.spyOn(republishHooks, 'useRepublishAdvertisement').mockReturnValue(
      asRepublishResult({ mutate: vi.fn(), isPending: false })
    );
    vi.spyOn(withdrawHooks, 'useWithdrawAdvertisement').mockReturnValue(
      asWithdrawResult({ mutate: vi.fn(), isPending: false })
    );
    mockRefresh();

    renderPage();

    expect(screen.getByRole('button', { name: 'Republish now' }).hasAttribute('disabled')).toBe(
      true
    );
    expect(screen.getByText('gateway admin_url did not respond')).toBeTruthy();
    expect(screen.getByText('no relays configured')).toBeTruthy();
  });
});
