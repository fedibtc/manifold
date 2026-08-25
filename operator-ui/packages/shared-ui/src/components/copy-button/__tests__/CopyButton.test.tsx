import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { CopyButton } from '../CopyButton';

describe('CopyButton', () => {
  const originalClipboard = navigator.clipboard;

  afterEach(() => {
    Object.assign(navigator, { clipboard: originalClipboard });
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
});
