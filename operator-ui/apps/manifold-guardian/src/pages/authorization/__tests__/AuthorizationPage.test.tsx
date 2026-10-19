import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MOCK_HOLDER_PUBKEY, MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';
import * as adminCallModule from '@/shared/api/adminCall';
import { AuthorizationPage } from '../AuthorizationPage';

const waiting = {
  fman_name: 'mutual-hamster',
  service_pubkey: '02abc',
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr: { state: 'not_observed', checked_at: 1_760_000_000 },
  fman_version: { current: '0.1.0', latest: null, update_required: false }
};

const observed = {
  ...waiting,
  nostr: {
    state: 'authorization_observed',
    authorizations: 1,
    holders: [MOCK_HOLDER_PUBKEY],
    checked_at: 1_760_000_000
  }
};

const renderPage = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <AuthorizationPage />
      </MemoryRouter>
    </QueryClientProvider>
  );
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('AuthorizationPage', () => {
  it('should show the waiting state with the full key', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(waiting);
    renderPage();

    await screen.findByText(MOCK_SERVICE_NOSTR_PUBKEY);
    expect(screen.getByText(/Not approved yet/i)).toBeTruthy();
  });

  // The daemon reports hex; a holder application shows the npub. The operator
  // compares the two screens, so this one renders theirs.
  it('should list an observed holder as the npub a holder application shows', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(observed);
    renderPage();

    await screen.findByText('npub1cswcupa4j23k78gvjnjcx7mz4uqet5l8c69jfg8huxw48jqzk6jqgqdz8m');
    expect(screen.getByText('Approved')).toBeTruthy();
  });

  it('should fall back to the reported value when a holder key does not encode', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
      ...waiting,
      nostr: {
        state: 'authorization_observed',
        authorizations: 1,
        holders: ['not-a-key'],
        checked_at: 1_760_000_000
      }
    });
    renderPage();

    await screen.findByText('not-a-key');
  });

  // The intro used to ask for a scan in both states, contradicting the
  // "Approved" banner rendered directly below it.
  it('should stop asking for a scan once the fleet is approved', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(observed);
    renderPage();

    await screen.findByText('Approved');
    expect(
      screen.queryByText(/Scan the code below with the PeerBadge app to approve it/i)
    ).toBeNull();
    expect(screen.getByText(/This host is approved\. The code below/i)).toBeTruthy();
  });

  it('should refresh authorization after approval without fetching relays on mount', async () => {
    const call = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(observed);
    renderPage();

    await screen.findByText('Approved');
    expect(call).not.toHaveBeenCalledWith('RefreshHolderAuthorizations');
    expect(screen.queryByRole('button', { name: 'Check now' })).toBeNull();

    call.mockResolvedValue({
      ...observed,
      nostr: { ...observed.nostr, holders: ['replacement-holder'] }
    });
    fireEvent.click(screen.getByRole('button', { name: 'Fetch new authorization' }));

    await screen.findByText('replacement-holder');
    expect(call).toHaveBeenCalledWith('RefreshHolderAuthorizations');
  });

  it('should show the fetch as busy and prevent a second click until it finishes', async () => {
    const call = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(observed);
    renderPage();
    await screen.findByText('Approved');

    let finishFetch!: (value: typeof observed) => void;
    call.mockReturnValueOnce(
      new Promise((resolve) => {
        finishFetch = resolve;
      })
    );
    const button = screen.getByRole('button', { name: 'Fetch new authorization' });
    fireEvent.click(button);

    await waitFor(() => expect(button).toBeDisabled());
    expect(button.getAttribute('aria-busy')).toBe('true');
    fireEvent.click(button);
    expect(
      call.mock.calls.filter(([request]) => request === 'RefreshHolderAuthorizations')
    ).toHaveLength(1);

    await act(async () => {
      finishFetch(observed);
    });
    await waitFor(() => expect(button).toBeEnabled());
    expect(button.getAttribute('aria-busy')).toBe('false');
  });

  it('should retain approval and report a failed replacement check', async () => {
    const call = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(observed);
    renderPage();

    await screen.findByText('Approved');
    call.mockRejectedValue(new Error('Relay unavailable'));
    fireEvent.click(screen.getByRole('button', { name: 'Fetch new authorization' }));

    await waitFor(() => expect(screen.getByText(/Relay unavailable/)).toBeTruthy());
    expect(screen.getByText('Approved')).toBeTruthy();
  });

  it('should open the PeerBadge guide from Show me how and close it again', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(observed);
    renderPage();

    await screen.findByText('Approved');
    const trigger = screen.getByRole('button', { name: 'Show me how' });
    trigger.focus();
    fireEvent.click(trigger);

    const dialog = screen.getByRole('dialog', { name: 'Renew your approval' });
    expect(within(dialog).getByRole('heading', { name: 'Open PeerBadge' })).toBeTruthy();
    expect(within(dialog).getByRole('heading', { name: 'Open your badge' })).toBeTruthy();
    expect(within(dialog).getByRole('heading', { name: 'Scan this code' })).toBeTruthy();

    fireEvent.click(within(dialog).getByRole('button', { name: 'Close' }));

    expect(screen.queryByRole('dialog')).toBeNull();
    expect(trigger).toHaveFocus();
  });

  it('should close the guide once a fetch from it succeeds', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(observed);
    renderPage();

    await screen.findByText('Approved');
    fireEvent.click(screen.getByRole('button', { name: 'Show me how' }));
    const dialog = screen.getByRole('dialog', { name: 'Renew your approval' });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Fetch new authorization' }));

    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  });

  it('should keep the guide open and show the error when a fetch from it fails', async () => {
    const call = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(observed);
    renderPage();

    await screen.findByText('Approved');
    call.mockRejectedValue(new Error('Relay unavailable'));
    fireEvent.click(screen.getByRole('button', { name: 'Show me how' }));
    const dialog = screen.getByRole('dialog', { name: 'Renew your approval' });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Fetch new authorization' }));

    await waitFor(() => expect(within(dialog).getByText(/Relay unavailable/)).toBeTruthy());
    expect(screen.getByRole('dialog', { name: 'Renew your approval' })).toBe(dialog);
  });

  it('should offer the guide beside the status, not in the update card, when the host is not approved', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(waiting);
    renderPage();

    await screen.findByText(/Not approved yet/i);
    const [trigger] = screen.getAllByRole('button', { name: 'Show me how' });
    const updateHeading = screen.getByRole('heading', { name: 'Update authorization' });

    expect(screen.getAllByRole('button', { name: 'Show me how' })).toHaveLength(1);
    expect(trigger.compareDocumentPosition(updateHeading)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
  });

  it('should title the guide as a first approval when the host is not approved', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(waiting);
    renderPage();

    await screen.findByText(/Not approved yet/i);
    fireEvent.click(screen.getByRole('button', { name: 'Show me how' }));

    expect(screen.getByRole('dialog', { name: 'Approve your host' })).toBeTruthy();
  });

  it('should link the guardian terms of service without an acceptance date', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(observed);
    renderPage();

    await screen.findByText('Approved');
    expect(screen.getByRole('heading', { name: 'Terms of service' })).toBeTruthy();
    expect(
      screen
        .getByRole('link', { name: /public\.qgcut\.org\/Fedi-verified_Guardian_ToS\.html/ })
        .getAttribute('href')
    ).toBe('https://public.qgcut.org/Fedi-verified_Guardian_ToS.html');
    expect(screen.queryByText(/accepted/i)).toBeNull();
  });

  it('should offer no way to skip or continue', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(waiting);
    renderPage();

    await screen.findByText(MOCK_SERVICE_NOSTR_PUBKEY);
    expect(screen.queryByRole('button', { name: /skip/i })).toBeNull();
    expect(screen.queryByRole('button', { name: /continue/i })).toBeNull();
  });
});
