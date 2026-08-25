import type { OnboardingResponse } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { MOCK_HOLDER_PUBKEY, MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';
import { AdminApiError, NetworkError } from '@/shared/api/errors';

// The encoded value is the whole point of the QR and is otherwise unreadable
// from the rendered SVG, so the code stands in for the real renderer.
vi.mock('qrcode.react', () => ({
  QRCodeSVG: ({ value }: { value: string }) => <div data-testid="qr">{value}</div>
}));

import { AuthorizationPanel } from '../AuthorizationPanel';

const waiting: OnboardingResponse = {
  stage: 'holder_authorization',
  runtime: 'starting',
  fman_name: 'mutual-hamster',
  service_pubkey: '02abc',
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr: { state: 'not_observed', checked_at: 1_760_000_000 },
  fman_version: { current: '0.1.0', latest: '0.1.0', update_required: false }
};

const observed: OnboardingResponse = {
  ...waiting,
  nostr: {
    state: 'authorization_observed',
    authorizations: 1,
    holders: [MOCK_HOLDER_PUBKEY],
    checked_at: 1_760_000_000
  }
};

describe('AuthorizationPanel', () => {
  it('should render the service nostr key in full', () => {
    render(<AuthorizationPanel data={waiting} isLoading={false} error={null} />);

    expect(screen.getByText(MOCK_SERVICE_NOSTR_PUBKEY)).toBeTruthy();
  });

  it('should encode a holder-parsable authorization request in the QR', () => {
    render(<AuthorizationPanel data={waiting} isLoading={false} error={null} />);

    expect(screen.getByTestId('qr').textContent).toBe(
      `{"subject_pubkey":"${MOCK_SERVICE_NOSTR_PUBKEY}"}`
    );
  });

  it('should offer a copy control for the request a holder pastes', () => {
    render(<AuthorizationPanel data={waiting} isLoading={false} error={null} />);

    expect(screen.getByRole('button', { name: /copy the authorization request/i })).toBeTruthy();
  });

  it('should not claim a holder app can scan and finish the flow', () => {
    render(<AuthorizationPanel data={waiting} isLoading={false} error={null} />);

    expect(screen.queryByText(/scans this with their app/i)).toBeNull();
  });

  it('should show a loading state instead of waiting text before the first response', () => {
    render(<AuthorizationPanel data={undefined} isLoading error={null} />);

    expect(screen.getByText(/reading the authorization state/i)).toBeTruthy();
    expect(screen.queryByText(/no authorization has been observed/i)).toBeNull();
  });

  it('should show the error state when it has no data at all', () => {
    render(
      <AuthorizationPanel
        data={undefined}
        isLoading={false}
        error={new AdminApiError('relay unavailable')}
      />
    );

    expect(screen.getByText('relay unavailable')).toBeTruthy();
    expect(screen.queryByText(/no authorization has been observed/i)).toBeNull();
  });

  it('should keep the last known state and warn when a refresh fails', () => {
    render(<AuthorizationPanel data={observed} isLoading={false} error={new NetworkError()} />);

    expect(screen.getByText(MOCK_SERVICE_NOSTR_PUBKEY)).toBeTruthy();
    expect(screen.getByText(/authorization observed/i)).toBeTruthy();
    expect(screen.getByText(/could not be refreshed/i)).toBeTruthy();
  });

  // The daemon now separates a completed read from no read, so the panel states
  // the result of the read instead of hedging about what it might still be doing.
  it('should report a completed read that found no authorization', () => {
    render(<AuthorizationPanel data={waiting} isLoading={false} error={null} />);

    expect(screen.getByText(/no authorization for this fleet/i)).toBeTruthy();
  });
});
