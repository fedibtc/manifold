import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

vi.mock('qrcode.react', () => ({
  QRCodeSVG: ({ value, size }: { value: string; size: number }) => (
    <div data-testid="qr" data-size={size}>
      {value}
    </div>
  )
}));

import { AuthorizationQrCode } from '../AuthorizationQrCode';

describe('AuthorizationQrCode', () => {
  it('should encode the request at the size it is given', () => {
    render(<AuthorizationQrCode request='{"subject_pubkey":"abc"}' size={128} />);

    expect(screen.getByTestId('qr').textContent).toBe('{"subject_pubkey":"abc"}');
    expect(screen.getByTestId('qr').dataset.size).toBe('128');
  });
});
