import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { DialogProgress } from '../DialogProgress';

describe('DialogProgress', () => {
  it('should tick finished steps and announce the running one', () => {
    render(
      <DialogProgress
        steps={[
          { label: 'Starting the payment', done: true },
          { label: 'Paying a@b.c over Lightning', done: false }
        ]}
      />
    );

    const list = screen.getByRole('list');
    expect(list).toHaveAttribute('aria-live', 'polite');
    const [started, paying] = screen.getAllByRole('listitem');
    expect(started).toHaveTextContent('✓Starting the payment');
    expect(paying).toHaveTextContent(/^Paying a@b.c over Lightning$/);
  });
});
