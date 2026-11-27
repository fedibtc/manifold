import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { MethodSwitch } from '../MethodSwitch';

describe('MethodSwitch', () => {
  it('should check the chosen method and keep only it in the tab order', () => {
    render(<MethodSwitch value="ecash" onChange={vi.fn()} />);

    expect(screen.getByRole('radio', { name: 'Ecash' })).toBeChecked();
    expect(screen.getByRole('radio', { name: 'Lightning' })).not.toBeChecked();
    expect(screen.getByRole('radio', { name: 'Ecash' })).toHaveAttribute('tabindex', '0');
    expect(screen.getByRole('radio', { name: 'Lightning' })).toHaveAttribute('tabindex', '-1');
  });

  it('should move to the other method with an arrow key and on click', () => {
    const onChange = vi.fn();
    render(<MethodSwitch value="lightning" onChange={onChange} />);

    fireEvent.keyDown(screen.getByRole('radiogroup', { name: 'How to withdraw' }), {
      key: 'ArrowRight'
    });
    fireEvent.click(screen.getByRole('radio', { name: 'Lightning' }));

    expect(onChange.mock.calls).toEqual([['ecash'], ['lightning']]);
    expect(screen.getByRole('radio', { name: 'Ecash' })).toHaveFocus();
  });
});
