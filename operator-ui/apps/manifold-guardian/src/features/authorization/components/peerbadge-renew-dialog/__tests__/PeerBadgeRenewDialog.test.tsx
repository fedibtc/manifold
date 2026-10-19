import type { OnboardingResponse } from '@operator-ui/types';
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';
import { PeerBadgeRenewDialog } from '../PeerBadgeRenewDialog';

const data: OnboardingResponse = {
  stage: 'complete',
  runtime: 'ready',
  fman_name: 'mutual-hamster',
  service_pubkey: '02abc',
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr: { state: 'not_observed', checked_at: 1_760_000_000 },
  fman_version: { current: '0.1.0', latest: '0.1.0', update_required: false }
};

const renderDialog = (isFetching = false, error: unknown = null) => {
  const onFetch = vi.fn();
  const onClose = vi.fn();
  render(
    <PeerBadgeRenewDialog
      data={data}
      isFetching={isFetching}
      error={error}
      onFetch={onFetch}
      onClose={onClose}
    />
  );
  return { onFetch, onClose };
};

describe('PeerBadgeRenewDialog', () => {
  it('should be named by its title and take focus', () => {
    renderDialog();

    expect(screen.getByRole('dialog')).toHaveAccessibleName('Renew your approval');
    expect(screen.getByRole('button', { name: 'Close' })).toHaveFocus();
  });

  it('should fetch from its primary action', () => {
    const { onFetch } = renderDialog();

    fireEvent.click(screen.getByRole('button', { name: 'Fetch new authorization' }));
    expect(onFetch).toHaveBeenCalledOnce();
  });

  it('should mark the fetch busy while it runs', () => {
    renderDialog(true);

    expect(
      screen.getByRole('button', { name: 'Fetch new authorization' }).getAttribute('aria-busy')
    ).toBe('true');
  });

  it('should show a failed fetch inside the dialog', () => {
    renderDialog(false, new Error('Relay unavailable'));

    expect(screen.getByText(/Relay unavailable/)).toBeTruthy();
  });

  it('should close on Escape, on the backdrop and from Close, but not from inside the card', () => {
    const { onClose } = renderDialog();

    fireEvent.click(screen.getByRole('heading', { name: 'Renew your approval' }));
    expect(onClose).not.toHaveBeenCalled();

    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    fireEvent.click(screen.getByRole('dialog'));
    fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(onClose).toHaveBeenCalledTimes(3);
  });

  it('should wrap Shift+Tab from Close to its last action', () => {
    renderDialog();

    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Tab', shiftKey: true });

    expect(screen.getByRole('button', { name: 'Fetch new authorization' })).toHaveFocus();
  });
});
