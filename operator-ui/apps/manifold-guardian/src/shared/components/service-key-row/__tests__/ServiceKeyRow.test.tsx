import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';
import { ServiceKeyRow } from '../ServiceKeyRow';

describe('ServiceKeyRow', () => {
  const originalClipboard = navigator.clipboard;

  afterEach(() => {
    Object.assign(navigator, { clipboard: originalClipboard });
  });

  it('should show the key in full', () => {
    render(<ServiceKeyRow pubkey={MOCK_SERVICE_NOSTR_PUBKEY} />);

    expect(screen.getByText(MOCK_SERVICE_NOSTR_PUBKEY)).toBeTruthy();
  });

  it('should put the bare key on the clipboard', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    render(<ServiceKeyRow pubkey={MOCK_SERVICE_NOSTR_PUBKEY} />);

    fireEvent.click(screen.getByRole('button', { name: /copy the service nostr public key/i }));

    await waitFor(() => expect(writeText).toHaveBeenCalledWith(MOCK_SERVICE_NOSTR_PUBKEY));
  });
});
