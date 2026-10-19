import type { OnboardingResponse } from '@operator-ui/types';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';
import { AdminApiError, NetworkError } from '@/shared/api/errors';

vi.mock('qrcode.react', () => ({
  QRCodeSVG: ({ value }: { value: string }) => <div data-testid="qr">{value}</div>
}));

import { SetupAuthorizationPanel } from '../SetupAuthorizationPanel';

const waiting: OnboardingResponse = {
  stage: 'holder_authorization',
  runtime: 'starting',
  fman_name: 'mutual-hamster',
  service_pubkey: '02abc',
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr: { state: 'not_observed', checked_at: 1_760_000_000 },
  fman_version: { current: '0.1.0', latest: '0.1.0', update_required: false }
};

const checkStatus = <button type="button">Check status</button>;

describe('SetupAuthorizationPanel', () => {
  it('should render the service nostr key in full', () => {
    render(
      <SetupAuthorizationPanel
        data={waiting}
        isLoading={false}
        error={null}
        statusAction={checkStatus}
      />
    );

    expect(screen.getByText(MOCK_SERVICE_NOSTR_PUBKEY)).toBeTruthy();
  });

  it('should encode a holder-parsable authorization request in the QR', () => {
    render(
      <SetupAuthorizationPanel
        data={waiting}
        isLoading={false}
        error={null}
        statusAction={checkStatus}
      />
    );

    expect(screen.getByTestId('qr').textContent).toBe(
      `{"subject_pubkey":"${MOCK_SERVICE_NOSTR_PUBKEY}"}`
    );
  });

  it('should name the request copy control on screen, not just to a reader', () => {
    render(
      <SetupAuthorizationPanel
        data={waiting}
        isLoading={false}
        error={null}
        statusAction={checkStatus}
      />
    );

    expect(screen.getByText('Copy the authorization request')).toBeTruthy();
  });

  describe('what each control puts on the clipboard', () => {
    const originalClipboard = navigator.clipboard;

    afterEach(() => {
      Object.assign(navigator, { clipboard: originalClipboard });
      vi.restoreAllMocks();
    });

    const clickAndReadClipboard = async (name: RegExp) => {
      const writeText = vi.fn().mockResolvedValue(undefined);
      Object.assign(navigator, { clipboard: { writeText } });
      render(
        <SetupAuthorizationPanel
          data={waiting}
          isLoading={false}
          error={null}
          statusAction={checkStatus}
        />
      );

      fireEvent.click(screen.getByRole('button', { name }));

      await waitFor(() => expect(writeText).toHaveBeenCalled());
      return writeText.mock.calls[0][0];
    };

    it('should put the bare key on the clipboard, not a JSON envelope', async () => {
      const copied = await clickAndReadClipboard(/copy the service nostr public key/i);

      expect(copied).toBe(MOCK_SERVICE_NOSTR_PUBKEY);
      expect(copied).not.toContain('subject_pubkey');
    });

    it('should put the parsable request on the clipboard from the request control', async () => {
      const copied = await clickAndReadClipboard(/copy the authorization request/i);

      expect(copied).toBe(`{"subject_pubkey":"${MOCK_SERVICE_NOSTR_PUBKEY}"}`);
    });
  });

  it('should walk the operator through the three PeerBadge steps', () => {
    render(
      <SetupAuthorizationPanel
        data={waiting}
        isLoading={false}
        error={null}
        statusAction={checkStatus}
      />
    );

    expect(screen.getByRole('heading', { name: 'Open your badge' })).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Tap Authorize application' })).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Scan this code' })).toBeTruthy();
    expect(screen.getByText('Then tap Authorize and publish.')).toBeTruthy();
    expect(screen.queryByText(/(?<!Manifold Fedimint )guardian ID/i)).toBeNull();
  });

  it('should show the re-check control inside the waiting status', () => {
    render(
      <SetupAuthorizationPanel
        data={waiting}
        isLoading={false}
        error={null}
        statusAction={checkStatus}
      />
    );

    const status = screen.getByRole('heading', { name: 'Waiting for approval' }).closest('section');
    expect(status?.textContent).toContain('Check status');
  });

  it('should show a loading state instead of waiting text before the first response', () => {
    render(
      <SetupAuthorizationPanel data={undefined} isLoading error={null} statusAction={checkStatus} />
    );

    expect(screen.getByText(/reading the authorization state/i)).toBeTruthy();
    expect(screen.queryByText(/Waiting for approval/i)).toBeNull();
  });

  it('should show the error state when it has no data at all', () => {
    render(
      <SetupAuthorizationPanel
        data={undefined}
        isLoading={false}
        error={new AdminApiError('relay unavailable')}
        statusAction={checkStatus}
      />
    );

    expect(screen.getByText('relay unavailable')).toBeTruthy();
    expect(screen.queryByText(/Waiting for approval/i)).toBeNull();
  });

  it('should keep the last known state and warn when a refresh fails', () => {
    render(
      <SetupAuthorizationPanel
        data={waiting}
        isLoading={false}
        error={new NetworkError()}
        statusAction={checkStatus}
      />
    );

    expect(screen.getByText(MOCK_SERVICE_NOSTR_PUBKEY)).toBeTruthy();
    expect(screen.getByText(/Waiting for approval/i)).toBeTruthy();
    expect(screen.getByText(/could not be refreshed/i)).toBeTruthy();
  });
});
