import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { CopyButton } from '../CopyButton';

describe('CopyButton', () => {
  const originalClipboard = navigator.clipboard;
  const originalExecCommand = document.execCommand;

  afterEach(() => {
    Object.assign(navigator, { clipboard: originalClipboard });
    Object.assign(document, { execCommand: originalExecCommand });
    vi.restoreAllMocks();
  });

  it('should expose an accessible label', () => {
    Object.assign(navigator, { clipboard: { writeText: vi.fn() } });
    render(<CopyButton value="abc123" label="Copy federation ID" />);

    expect(screen.getByRole('button', { name: 'Copy federation ID' })).toBeTruthy();
  });

  it('should copy the full value to the clipboard on click', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    render(<CopyButton value="abc123" label="Copy federation ID" />);

    fireEvent.click(screen.getByRole('button', { name: 'Copy federation ID' }));

    await waitFor(() => expect(writeText).toHaveBeenCalledWith('abc123'));
  });

  it('should mark itself copied after a successful copy', async () => {
    Object.assign(navigator, { clipboard: { writeText: vi.fn().mockResolvedValue(undefined) } });
    render(<CopyButton value="abc123" label="Copy federation ID" />);

    const button = screen.getByRole('button', { name: 'Copy federation ID' });
    fireEvent.click(button);

    await waitFor(() => expect(button.getAttribute('data-copied')).toBe('true'));
  });

  // A fleet manager is commonly reached over plain http, where
  // `navigator.clipboard` does not exist. The control used to do nothing at all
  // there, which reads as broken.
  it('should fall back to a selection copy when the clipboard API is absent', async () => {
    Object.assign(navigator, { clipboard: undefined });
    const execCommand = vi.fn().mockReturnValue(true);
    Object.assign(document, { execCommand });
    render(<CopyButton value="abc123" label="Copy federation ID" />);

    const button = screen.getByRole('button', { name: 'Copy federation ID' });
    fireEvent.click(button);

    await waitFor(() => expect(execCommand).toHaveBeenCalledWith('copy'));
    await waitFor(() => expect(button.getAttribute('data-copied')).toBe('true'));
  });

  it('should report a failure rather than doing nothing silently', async () => {
    Object.assign(navigator, { clipboard: { writeText: vi.fn().mockRejectedValue(new Error()) } });
    Object.assign(document, { execCommand: vi.fn().mockReturnValue(false) });
    render(<CopyButton value="abc123" label="Copy federation ID" />);

    fireEvent.click(screen.getByRole('button', { name: 'Copy federation ID' }));

    await waitFor(() =>
      expect(screen.getByRole('button', { name: /copying failed/i }).dataset.failed).toBe('true')
    );
  });

  it('should show the label on screen when asked to', () => {
    Object.assign(navigator, { clipboard: { writeText: vi.fn() } });
    render(<CopyButton value="abc123" label="Copy the authorization request" showLabel />);

    expect(screen.getByText('Copy the authorization request')).toBeInTheDocument();
  });
});
