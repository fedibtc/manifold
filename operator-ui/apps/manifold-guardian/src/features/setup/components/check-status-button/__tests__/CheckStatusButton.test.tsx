import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { CheckStatusButton } from '../CheckStatusButton';

describe('CheckStatusButton', () => {
  it('should read the status when clicked', () => {
    const onClick = vi.fn();
    render(<CheckStatusButton loading={false} onClick={onClick} />);

    fireEvent.click(screen.getByRole('button', { name: 'Check status' }));

    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
