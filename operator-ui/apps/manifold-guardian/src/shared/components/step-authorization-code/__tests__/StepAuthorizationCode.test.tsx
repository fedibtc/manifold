import type { OnboardingResponse } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';

vi.mock('qrcode.react', () => ({
  QRCodeSVG: ({ value, size }: { value: string; size: number }) => (
    <div data-testid="qr" data-size={size}>
      {value}
    </div>
  )
}));

import { StepAuthorizationCode } from '../StepAuthorizationCode';

const data: OnboardingResponse = {
  stage: 'holder_authorization',
  runtime: 'starting',
  fman_name: 'mutual-hamster',
  service_pubkey: '02abc',
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr: { state: 'not_observed', checked_at: 1_760_000_000 },
  fman_version: { current: '0.1.0', latest: '0.1.0', update_required: false }
};

const follows = (earlier: Element, later: Element) =>
  Boolean(earlier.compareDocumentPosition(later) & Node.DOCUMENT_POSITION_FOLLOWING);

describe('StepAuthorizationCode', () => {
  it('should show a small QR, then the copy request control, then the labelled ID', () => {
    render(<StepAuthorizationCode data={data} />);
    const qr = screen.getByTestId('qr');
    const copy = screen.getByText('Copy the authorization request');
    const label = screen.getByText('Manifold Fedimint Guardian ID');
    const key = screen.getByText(MOCK_SERVICE_NOSTR_PUBKEY);

    expect(qr.dataset.size).toBe('128');
    expect(follows(qr, copy)).toBe(true);
    expect(follows(copy, label)).toBe(true);
    expect(follows(label, key)).toBe(true);
  });

  it('should offer the copy request as an alternative to scanning', () => {
    render(<StepAuthorizationCode data={data} />);

    expect(screen.getByText('or')).toBeTruthy();
  });
});
