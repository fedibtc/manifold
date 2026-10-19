import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { CopyAuthorizationRequest } from '../CopyAuthorizationRequest';

describe('CopyAuthorizationRequest', () => {
  const originalClipboard = navigator.clipboard;

  afterEach(() => {
    Object.assign(navigator, { clipboard: originalClipboard });
  });

  it('should name the control on screen', () => {
    render(<CopyAuthorizationRequest request='{"subject_pubkey":"abc"}' />);

    expect(screen.getByText('Copy the authorization request')).toBeTruthy();
  });

  it('should put the request on the clipboard', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    render(<CopyAuthorizationRequest request='{"subject_pubkey":"abc"}' />);

    fireEvent.click(screen.getByRole('button', { name: /copy the authorization request/i }));

    await waitFor(() => expect(writeText).toHaveBeenCalledWith('{"subject_pubkey":"abc"}'));
  });
});
