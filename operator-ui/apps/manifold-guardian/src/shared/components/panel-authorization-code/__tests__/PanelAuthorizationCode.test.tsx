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

import { PanelAuthorizationCode } from '../PanelAuthorizationCode';

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

describe('PanelAuthorizationCode', () => {
  it('should show a large QR, then the key, then the copy request control', () => {
    render(<PanelAuthorizationCode data={data} />);
    const qr = screen.getByTestId('qr');
    const key = screen.getByText(MOCK_SERVICE_NOSTR_PUBKEY);
    const copy = screen.getByText('Copy the authorization request');

    expect(qr.dataset.size).toBe('192');
    expect(follows(qr, key)).toBe(true);
    expect(follows(key, copy)).toBe(true);
  });
});
