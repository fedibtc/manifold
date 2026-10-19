import type { OnboardingResponse } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';

vi.mock('qrcode.react', () => ({
  QRCodeSVG: ({ value }: { value: string }) => <div data-testid="qr">{value}</div>
}));

import { PeerBadgeApprovalSteps } from '../PeerBadgeApprovalSteps';

const waiting: OnboardingResponse = {
  stage: 'holder_authorization',
  runtime: 'starting',
  fman_name: 'mutual-hamster',
  service_pubkey: '02abc',
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr: { state: 'not_observed', checked_at: 1_760_000_000 },
  fman_version: { current: '0.1.0', latest: '0.1.0', update_required: false }
};

describe('PeerBadgeApprovalSteps', () => {
  it('should render the three PeerBadge steps with the request QR and the full ID', () => {
    render(<PeerBadgeApprovalSteps data={waiting} />);

    expect(screen.getByRole('heading', { name: 'Open PeerBadge' })).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Open your badge' })).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Scan this code' })).toBeTruthy();
    expect(screen.getByTestId('qr').textContent).toBe(
      `{"subject_pubkey":"${MOCK_SERVICE_NOSTR_PUBKEY}"}`
    );
    expect(screen.getByText(MOCK_SERVICE_NOSTR_PUBKEY)).toBeTruthy();
  });

  it('should show the PeerBadge home screen first and no scanner screen', () => {
    render(<PeerBadgeApprovalSteps data={waiting} />);

    expect(screen.getByRole('img', { name: /What do you want to do/ })).toBeTruthy();
    expect(screen.getByRole('img', { name: /^My badges:/ })).toBeTruthy();
    expect(screen.queryByRole('img', { name: /scanner/ })).toBeNull();
  });
});
