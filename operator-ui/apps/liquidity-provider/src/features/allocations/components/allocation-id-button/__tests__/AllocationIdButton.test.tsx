import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { AllocationIdButton } from '@/features/allocations/components/allocation-id-button/AllocationIdButton';

describe('AllocationIdButton', () => {
  it('should render the id as the button label', () => {
    render(<AllocationIdButton id="ft-1" selected={false} onSelect={vi.fn()} />);
    expect(screen.getByRole('button', { name: 'ft-1' })).toBeTruthy();
  });

  it('should call onSelect with the id when clicked', () => {
    const onSelect = vi.fn();
    render(<AllocationIdButton id="ft-1" selected={false} onSelect={onSelect} />);

    fireEvent.click(screen.getByRole('button', { name: 'ft-1' }));

    expect(onSelect).toHaveBeenCalledWith('ft-1');
  });

  it('should mark the button as pressed when selected', () => {
    render(<AllocationIdButton id="ft-1" selected onSelect={vi.fn()} />);
    expect(screen.getByRole('button', { name: 'ft-1' }).getAttribute('aria-pressed')).toBe('true');
  });
});
