import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { StatusChip } from '../StatusChip';

describe('StatusChip', () => {
  it('should render its label', () => {
    render(<StatusChip tone="ok">Published</StatusChip>);
    expect(screen.getByText('Published')).toBeTruthy();
  });

  it('should expose the tone as a data attribute for styling', () => {
    render(<StatusChip tone="bad">Disconnected</StatusChip>);
    expect(screen.getByText('Disconnected').getAttribute('data-tone')).toBe('bad');
  });
});
