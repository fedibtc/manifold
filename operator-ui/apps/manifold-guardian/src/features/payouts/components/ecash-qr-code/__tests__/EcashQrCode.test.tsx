import { act, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { EcashQrCode, MAX_SINGLE_QR_TOKEN_LENGTH, QR_FRAME_MS } from '../EcashQrCode';

const drawing = (name: string) =>
  screen.getByRole('img', { name }).querySelector('path:last-of-type')?.getAttribute('d');

afterEach(() => {
  vi.useRealTimers();
});

describe('EcashQrCode', () => {
  it('should draw a short token as one still code', () => {
    vi.useFakeTimers();
    render(<EcashQrCode token={`fedimint${'a'.repeat(MAX_SINGLE_QR_TOKEN_LENGTH - 8)}`} />);
    const first = drawing('Ecash QR code');
    expect(first).toEqual(expect.any(String));

    act(() => vi.advanceTimersByTime(QR_FRAME_MS * 3));

    expect(drawing('Ecash QR code')).toBe(first);
  });

  // One code for a whole-balance token is too dense to scan off a screen; the
  // Fedi scanner reassembles a loop of smaller ones.
  it('should loop a long token through frames', () => {
    vi.useFakeTimers();
    render(<EcashQrCode token={`fedimint${'a1b2c3d4'.repeat(200)}`} />);
    const first = drawing('Ecash QR code, animated');

    act(() => vi.advanceTimersByTime(QR_FRAME_MS));

    expect(drawing('Ecash QR code, animated')).not.toBe(first);
  });
});
